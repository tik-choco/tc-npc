package audio

import (
	"errors"
	"fmt"
	"log/slog"
	"runtime"
	"strings"
	"syscall"
	"time"
	"unsafe"

	"github.com/go-ole/go-ole"
	"github.com/moutend/go-wca/pkg/wca"
	"golang.org/x/sys/windows"
)

const (
	endpointNotFound     = 0x80070490
	captureBufferEmpty   = 0x08890001
	variantWideString    = 31 // VT_LPWSTR
	wasapiBufferDuration = 40 * time.Millisecond
)

var clearPropVariant = windows.NewLazySystemDLL("ole32.dll").NewProc("PropVariantClear")

// ListInputDevices lists unique named active input endpoints in Windows order.
func ListInputDevices() ([]DeviceInfo, error) { return listDevices(wca.ECapture) }

// ListOutputDevices lists unique named active output endpoints in Windows order.
func ListOutputDevices() ([]DeviceInfo, error) { return listDevices(wca.ERender) }

func comCodeIs(err error, code uint32) bool {
	var oleErr *ole.OleError
	return errors.As(err, &oleErr) && uint32(oleErr.Code()) == code
}

// All COM objects acquired by use must be released before it returns.
func withDeviceEnumerator(use func(*wca.IMMDeviceEnumerator) error) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	if err := ole.CoInitializeEx(0, ole.COINIT_MULTITHREADED); err != nil && !comCodeIs(err, 1) {
		return fmt.Errorf("initialize audio COM apartment: %w", err)
	}
	defer ole.CoUninitialize()
	var enumerator *wca.IMMDeviceEnumerator
	if err := wca.CoCreateInstance(wca.CLSID_MMDeviceEnumerator, 0, wca.CLSCTX_INPROC_SERVER,
		wca.IID_IMMDeviceEnumerator, &enumerator); err != nil {
		return fmt.Errorf("create audio device enumerator: %w", err)
	}
	defer enumerator.Release()
	return use(enumerator)
}

// Returning true from visit ends the walk. It must AddRef any device it keeps.
func walkEndpoints(enumerator *wca.IMMDeviceEnumerator, flow uint32, visit func(*wca.IMMDevice) bool) error {
	var collection *wca.IMMDeviceCollection
	if err := enumerator.EnumAudioEndpoints(flow, wca.DEVICE_STATE_ACTIVE, &collection); err != nil {
		return fmt.Errorf("enumerate active audio devices: %w", err)
	}
	defer collection.Release()
	var count uint32
	if err := collection.GetCount(&count); err != nil {
		return fmt.Errorf("count active audio devices: %w", err)
	}
	for i := range count {
		var device *wca.IMMDevice
		if err := collection.Item(i, &device); err != nil {
			continue
		}
		stop := visit(device)
		device.Release()
		if stop {
			break
		}
	}
	return nil
}

func endpointName(device *wca.IMMDevice) string {
	var store *wca.IPropertyStore
	if err := device.OpenPropertyStore(wca.STGM_READ, &store); err != nil {
		return ""
	}
	defer store.Release()
	var value wca.PROPVARIANT
	if err := store.GetValue(&wca.PKEY_Device_FriendlyName, &value); err != nil {
		return ""
	}
	defer func() { _, _, _ = clearPropVariant.Call(uintptr(unsafe.Pointer(&value))) }()
	if value.VT != variantWideString {
		return ""
	}
	return windows.UTF16PtrToString(*(**uint16)(unsafe.Pointer(&value.Val)))
}

func endpointID(device *wca.IMMDevice) string {
	// go-wca's GetId dereferences the result even if IMMDevice::GetId fails.
	var id *uint16
	hr, _, _ := syscall.SyscallN(device.VTable().GetId, uintptr(unsafe.Pointer(device)), uintptr(unsafe.Pointer(&id)))
	if hr != ole.S_OK || id == nil {
		return ""
	}
	defer ole.CoTaskMemFree(uintptr(unsafe.Pointer(id)))
	return windows.UTF16PtrToString(id)
}

func defaultEndpoint(enumerator *wca.IMMDeviceEnumerator, flow uint32) (*wca.IMMDevice, error) {
	var device *wca.IMMDevice
	if err := enumerator.GetDefaultAudioEndpoint(flow, wca.EConsole, &device); err != nil {
		if comCodeIs(err, endpointNotFound) {
			return nil, ErrNoDevice
		}
		return nil, fmt.Errorf("get Windows default audio endpoint: %w", err)
	}
	return device, nil
}

func listDevices(flow uint32) ([]DeviceInfo, error) {
	var devices []DeviceInfo
	err := withDeviceEnumerator(func(enumerator *wca.IMMDeviceEnumerator) error {
		var defaultID string
		def, err := defaultEndpoint(enumerator, flow)
		if err == nil {
			defaultID = endpointID(def)
			def.Release()
		} else if !errors.Is(err, ErrNoDevice) {
			return err
		}
		return walkEndpoints(enumerator, flow, func(device *wca.IMMDevice) bool {
			devices = append(devices, DeviceInfo{
				Name:      endpointName(device),
				IsDefault: defaultID != "" && endpointID(device) == defaultID,
			})
			return false
		})
	})
	if err != nil {
		return nil, err
	}
	return uniqueDeviceNames(devices), nil
}

func selectEndpoint(enumerator *wca.IMMDeviceEnumerator, flow uint32, name string) (*wca.IMMDevice, error) {
	needle := strings.ToLower(strings.TrimSpace(name))
	if needle != "" {
		var selected *wca.IMMDevice
		// A failed enumeration also falls back to the default, as in cpal.
		_ = walkEndpoints(enumerator, flow, func(device *wca.IMMDevice) bool {
			if !matchesDeviceName(endpointName(device), needle) {
				return false
			}
			device.AddRef()
			selected = device
			return true
		})
		if selected != nil {
			return selected, nil
		}
	}
	return defaultEndpoint(enumerator, flow)
}

type wasapiStream struct {
	client         *wca.IAudioClient
	rate, channels int
	floatSamples   bool
	poll           time.Duration
}

func withWASAPIStream(flow uint32, name string, run func(wasapiStream) error) error {
	return withDeviceEnumerator(func(enumerator *wca.IMMDeviceEnumerator) error {
		device, err := selectEndpoint(enumerator, flow, name)
		if err != nil {
			return err
		}
		defer device.Release()

		var client *wca.IAudioClient
		// go-wca's Activate passes the CLSCTX value by address. The native
		// method takes that DWORD by value.
		hr, _, _ := syscall.SyscallN(device.VTable().Activate,
			uintptr(unsafe.Pointer(device)), uintptr(unsafe.Pointer(wca.IID_IAudioClient)),
			wca.CLSCTX_ALL, 0, uintptr(unsafe.Pointer(&client)))
		if hr != ole.S_OK {
			return fmt.Errorf("activate WASAPI audio client: %w", ole.NewError(hr))
		}
		defer client.Release()
		stream, err := initializeWASAPI(client)
		if err != nil {
			return err
		}
		return run(stream)
	})
}

func initializeWASAPI(client *wca.IAudioClient) (wasapiStream, error) {
	var f *wca.WAVEFORMATEX
	if err := client.GetMixFormat(&f); err != nil {
		return wasapiStream{}, fmt.Errorf("get WASAPI endpoint mix format: %w", err)
	}
	defer ole.CoTaskMemFree(uintptr(unsafe.Pointer(f)))

	tag := f.WFormatTag
	if tag == waveExtensible && f.CbSize >= 22 {
		// WAVEFORMATEXTENSIBLE is packed. Its GUID is at byte 24 of
		// WAVEFORMATEX, whose Go struct has trailing padding.
		extension := unsafe.Slice((*byte)(unsafe.Pointer(f)), 40)
		if string(extension[26:40]) == waveSubtypeSuffix && extension[25] == 0 {
			tag = uint16(extension[24])
		}
	}
	isFloat := tag == waveFloat && f.WBitsPerSample == 32
	isPCM16 := tag == wavePCM && f.WBitsPerSample == 16
	if (!isFloat && !isPCM16) || f.NChannels == 0 || int(f.NSamplesPerSec) <= 0 ||
		uint32(f.NBlockAlign)*8 != uint32(f.WBitsPerSample)*uint32(f.NChannels) {
		return wasapiStream{}, fmt.Errorf("unsupported WASAPI mix format 0x%x (%d-bit, %d channels, %d Hz)",
			tag, f.WBitsPerSample, f.NChannels, f.NSamplesPerSec)
	}
	var period wca.REFERENCE_TIME
	if err := client.GetDevicePeriod(&period, nil); err != nil {
		return wasapiStream{}, fmt.Errorf("get WASAPI endpoint period: %w", err)
	}
	poll := min(5*time.Millisecond, max(time.Millisecond, time.Duration(period)*100*time.Nanosecond/2))
	if err := initializeShared(client, f); err != nil {
		return wasapiStream{}, fmt.Errorf("initialize WASAPI shared stream: %w", err)
	}
	return wasapiStream{client, int(f.NSamplesPerSec), int(f.NChannels), isFloat, poll}, nil
}

func initializeShared(client *wca.IAudioClient, format *wca.WAVEFORMATEX) error {
	duration := wca.REFERENCE_TIME(wasapiBufferDuration / (100 * time.Nanosecond))
	if runtime.GOARCH != "386" {
		return client.Initialize(wca.AUDCLNT_SHAREMODE_SHARED, 0, duration, 0, format, nil)
	}
	// Both REFERENCE_TIME arguments occupy two stack words on 32-bit Windows.
	hr, _, _ := syscall.SyscallN(client.VTable().Initialize, uintptr(unsafe.Pointer(client)),
		wca.AUDCLNT_SHAREMODE_SHARED, 0, uintptr(duration), 0, 0, 0, uintptr(unsafe.Pointer(format)), 0)
	if hr != ole.S_OK {
		return ole.NewError(hr)
	}
	return nil
}

// OpenCapture starts capture on the first active endpoint whose name contains
// the trimmed, case-insensitive deviceName. An empty or unmatched name uses
// the Windows default endpoint. onData runs synchronously on the audio worker
// and may start before OpenCapture returns. Copy the slice to retain it and
// return promptly. Close capture from outside the callback.
func OpenCapture(deviceName string, onData func(interleaved []float32)) (Capture, error) {
	if onData == nil {
		return nil, errors.New("capture requires an audio callback")
	}
	capture := &streamState{}
	err := capture.open(func(ready func(int, int)) error {
		return withWASAPIStream(wca.ECapture, deviceName, func(stream wasapiStream) error {
			return stream.capture(capture.stopping, ready, onData)
		})
	})
	if err != nil {
		return nil, fmt.Errorf("open audio capture: %w", err)
	}
	return capture, nil
}

func (s wasapiStream) capture(stopping <-chan struct{}, ready func(int, int), onData func([]float32)) (err error) {
	var capture *wca.IAudioCaptureClient
	if err := s.client.GetService(wca.IID_IAudioCaptureClient, &capture); err != nil {
		return fmt.Errorf("get WASAPI capture service: %w", err)
	}
	defer capture.Release()
	if err := s.client.Start(); err != nil {
		return fmt.Errorf("start WASAPI capture: %w", err)
	}
	defer func() {
		if stopErr := s.client.Stop(); stopErr != nil {
			err = errors.Join(err, fmt.Errorf("stop WASAPI capture: %w", stopErr))
		}
		if err != nil {
			slog.Error("audio capture stream stopped", "error", err)
		}
	}()
	ready(s.rate, s.channels)
	ticker := time.NewTicker(s.poll)
	defer ticker.Stop()
	var samples []float32
	for {
		select {
		case <-stopping:
			return nil
		case <-ticker.C:
		}
		for {
			select {
			case <-stopping:
				return nil
			default:
			}
			var next uint32
			if err := capture.GetNextPacketSize(&next); err != nil {
				return fmt.Errorf("poll WASAPI capture: %w", err)
			}
			if next == 0 {
				break
			}
			var data *byte
			var frames, flags uint32
			if err := capture.GetBuffer(&data, &frames, &flags, nil, nil); err != nil {
				if comCodeIs(err, captureBufferEmpty) {
					break
				}
				return fmt.Errorf("read WASAPI capture packet: %w", err)
			}
			count := int(frames) * s.channels
			if cap(samples) < count {
				samples = make([]float32, count)
			}
			samples = samples[:count]
			switch {
			case flags&wca.AUDCLNT_BUFFERFLAGS_SILENT != 0:
				clear(samples)
			case s.floatSamples:
				copy(samples, unsafe.Slice((*float32)(unsafe.Pointer(data)), count))
			default:
				for i, sample := range unsafe.Slice((*int16)(unsafe.Pointer(data)), count) {
					samples[i] = float32(sample) / 32768
				}
			}
			if err := capture.ReleaseBuffer(frames); err != nil {
				return fmt.Errorf("release WASAPI capture packet: %w", err)
			}
			if len(samples) != 0 {
				onData(samples)
			}
		}
	}
}

// OpenPlayback selects a Windows output endpoint using the same name matching
// and default fallback as OpenCapture.
func OpenPlayback(deviceName string) (Playback, error) {
	playback := newPlaybackStream()
	err := playback.open(func(ready func(int, int)) error {
		return withWASAPIStream(wca.ERender, deviceName, func(stream wasapiStream) error {
			var render *wca.IAudioRenderClient
			if err := stream.client.GetService(wca.IID_IAudioRenderClient, &render); err != nil {
				return fmt.Errorf("get WASAPI render service: %w", err)
			}
			defer render.Release()
			var frames uint32
			if err := stream.client.GetBufferSize(&frames); err != nil {
				return fmt.Errorf("get WASAPI render buffer size: %w", err)
			}
			if int(frames) <= 0 {
				return fmt.Errorf("invalid WASAPI render buffer size of %d frames", frames)
			}
			var latency wca.REFERENCE_TIME
			if err := stream.client.GetStreamLatency(&latency); err != nil {
				return fmt.Errorf("get WASAPI render latency: %w", err)
			}
			ready(stream.rate, stream.channels)
			device := wasapiRender{stream, render}
			return playback.run(device, int(frames), stream.poll, time.Duration(latency)*100*time.Nanosecond)
		})
	})
	if err != nil {
		return nil, fmt.Errorf("open audio playback: %w", err)
	}
	return playback, nil
}

type wasapiRender struct {
	wasapiStream
	render *wca.IAudioRenderClient
}

func (s wasapiRender) padding() (int, error) {
	var frames uint32
	if err := s.client.GetCurrentPadding(&frames); err != nil {
		return 0, fmt.Errorf("poll WASAPI render padding: %w", err)
	}
	return int(frames), nil
}

func (s wasapiRender) queue(samples []float32) error {
	frames := uint32(len(samples) / s.channels)
	var data *byte
	if err := s.render.GetBuffer(frames, &data); err != nil {
		return fmt.Errorf("get WASAPI render buffer: %w", err)
	}
	if s.floatSamples {
		copy(unsafe.Slice((*float32)(unsafe.Pointer(data)), len(samples)), samples)
	} else {
		out := unsafe.Slice((*int16)(unsafe.Pointer(data)), len(samples))
		for i, sample := range samples {
			out[i] = normalizedToPCM16(sample)
		}
	}
	if err := s.render.ReleaseBuffer(frames, 0); err != nil {
		return fmt.Errorf("submit WASAPI render buffer: %w", err)
	}
	return nil
}

func (s wasapiRender) start() error { return renderControl("start", s.client.Start()) }
func (s wasapiRender) stop() error  { return renderControl("stop", s.client.Stop()) }
func (s wasapiRender) reset() error { return renderControl("reset", s.client.Reset()) }

func renderControl(operation string, err error) error {
	if err != nil {
		return fmt.Errorf("%s WASAPI playback: %w", operation, err)
	}
	return nil
}
