package mist

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

const (
	mistDLLName = "mistlib.dll"
	mistDLLEnv  = "TC_NPC_MISTLIB_DLL"
	reliable    = 0
	unreliable  = 2
)

var (
	engineStarted atomic.Bool
	emptyDLLByte  byte
)

type nativeEngine struct {
	init, join, send, get, leave *windows.LazyProc
	registerEvent, registerLog   *windows.LazyProc
	getVersion, getBuildInfo     *windows.LazyProc
	events                       atomic.Pointer[engineEvents]
}

func loadEngine(dataDir string) (engine, error) {
	exe, _ := os.Executable()
	path, err := findDLL(os.Getenv(mistDLLEnv), exe, dataDir)
	if err != nil {
		return nil, err
	}
	return openDLL(path)
}

func findDLL(override, exe, dataDir string) (string, error) {
	candidates := []string{override}
	if exe != "" {
		candidates = append(candidates, filepath.Join(filepath.Dir(exe), mistDLLName))
	}
	dataPath := filepath.Join(dataDir, mistDLLName)
	candidates = append(candidates, dataPath)
	for _, candidate := range candidates {
		if candidate == "" {
			continue
		}
		path, err := filepath.Abs(candidate)
		if err != nil {
			return "", err
		}
		info, err := os.Stat(path)
		if errors.Is(err, os.ErrNotExist) || (err == nil && !info.Mode().IsRegular()) {
			continue
		}
		if err != nil {
			return "", fmt.Errorf("npc-mist: check DLL path %q: %w", path, err)
		}
		return path, nil
	}
	return "", fmt.Errorf("npc-mist: mistlib.dll not found; set %s to its path, or put it next to the executable or at %s", mistDLLEnv, dataPath)
}

func openDLL(path string) (*nativeEngine, error) {
	dll := windows.NewLazyDLL(path)
	if err := dll.Load(); err != nil {
		return nil, fmt.Errorf("npc-mist: load %q: %w", path, err)
	}
	e := &nativeEngine{
		init: dll.NewProc("init_with_config"), join: dll.NewProc("join_room"),
		send: dll.NewProc("send_message_in_room"), get: dll.NewProc("storage_get"),
		leave: dll.NewProc("leave_all_rooms"), registerEvent: dll.NewProc("register_event_callback_v2"),
		registerLog: dll.NewProc("register_log_callback"),
		getVersion:  dll.NewProc("get_version"), getBuildInfo: dll.NewProc("get_build_info"),
	}
	for _, proc := range []*windows.LazyProc{
		e.init, e.join, e.send, e.get, e.leave, e.registerEvent, e.registerLog, e.getVersion, e.getBuildInfo,
	} {
		if err := proc.Find(); err != nil {
			_ = windows.FreeLibrary(windows.Handle(dll.Handle()))
			return nil, fmt.Errorf("npc-mist: resolve mistlib exports in %q: %w", path, err)
		}
	}
	// mistlib owns process-wide workers and retains callback addresses. Keep
	// the loaded DLL resident for the lifetime of the process.
	return e, nil
}

func (e *nativeEngine) start(nodeID, signalingURL string) (*engineEvents, error) {
	config, err := json.Marshal(makeEngineConfig(signalingURL))
	if err != nil {
		return nil, fmt.Errorf("mist: encode signaling config: %w", err)
	}
	if !engineStarted.CompareAndSwap(false, true) {
		return nil, errors.New("mist engine already started in this process")
	}
	id := []byte(nodeID)
	result, _, _ := e.init.Call(uintptr(dllBytes(id)), uintptr(len(id)), uintptr(dllBytes(config)), uintptr(len(config)))
	if uint8(result) == 0 { // Rust bool is one byte.
		return nil, errors.New("mistlib rejected the signaling config")
	}
	events := newEngineEvents()
	e.events.Store(events)
	newCallback := syscall.NewCallback
	if runtime.GOARCH == "386" {
		newCallback = syscall.NewCallbackCDecl // Rust extern "C" uses cdecl on x86.
	}
	e.registerEvent.Call(newCallback(e.onEvent))
	e.registerLog.Call(newCallback(e.onLog))
	return events, nil
}

// C callback v2: event code, followed by the room, sender, and data pointer/length pairs.
func (e *nativeEngine) onEvent(kind uint32, room *byte, roomLen uintptr, _ *byte, _ uintptr, data *byte, dataLen uintptr) uintptr {
	if kind != eventRaw && kind != eventRoomJoined && kind != eventRoomJoinFailed {
		return 0
	}
	if events := e.events.Load(); events != nil {
		events.push(&roomEvent{
			kind: kind,
			room: string(unsafe.Slice(room, roomLen)),
			data: bytes.Clone(unsafe.Slice(data, dataLen)),
		})
	}
	return 0
}

func (e *nativeEngine) onLog(level uint32, data *byte, size uintptr) uintptr {
	if events := e.events.Load(); events != nil {
		levels := [...]slog.Level{slog.LevelDebug, slog.LevelInfo, slog.LevelWarn, slog.LevelError}
		if level >= uint32(len(levels)) {
			return 0
		}
		entry := nativeLog{levels[level], string(unsafe.Slice(data, size))}
		select {
		case events.logs <- entry:
		default:
		}
	}
	return 0
}

func (e *nativeEngine) joinRoom(room string) {
	id := []byte(room)
	e.join.Call(uintptr(dllBytes(id)), uintptr(len(id)))
}

func (e *nativeEngine) broadcast(room string, data []byte, lossy bool) {
	id := []byte(room)
	delivery := uintptr(reliable)
	if lossy {
		delivery = unreliable
	}
	// An empty target broadcasts only in this room. The C export reports send
	// queue failures through mistlib logging and has no return status.
	e.send.Call(uintptr(dllBytes(id)), uintptr(len(id)), uintptr(dllBytes(nil)), 0,
		uintptr(dllBytes(data)), uintptr(len(data)), delivery)
}

func (e *nativeEngine) storageGet(ctx context.Context, cid string) ([]byte, error) {
	id := []byte(cid)
	// Small character payloads fit on the first call. A short buffer reports
	// the required length without copying. Each call can block in mistlib.
	data, err := readDLLBytes(4096, func(buffer []byte) uint32 {
		if ctx.Err() != nil {
			return 0
		}
		n, _, _ := e.get.Call(uintptr(dllBytes(id)), uintptr(len(id)),
			uintptr(dllBytes(buffer)), uintptr(len(buffer)))
		return uint32(n)
	})
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}
	if err != nil {
		return nil, fmt.Errorf("mist: storage_get(%q): %w", cid, err)
	}
	return data, nil
}

func (e *nativeEngine) leaveAllRooms() {
	e.events.Store(nil)
	// A Go caller is outside the DLL's Tokio runtime, so this waits for teardown.
	e.leave.Call()
}

func dllBytes(data []byte) unsafe.Pointer {
	if len(data) == 0 {
		// Rust slices require non-null pointers even when their length is zero.
		return unsafe.Pointer(&emptyDLLByte)
	}
	return unsafe.Pointer(unsafe.SliceData(data))
}

func readDLLBytes(capacity int, call func([]byte) uint32) ([]byte, error) {
	buffer := make([]byte, capacity)
	for {
		n := call(buffer)
		switch {
		case n == 0:
			return nil, errors.New("mistlib returned zero bytes")
		case uint64(n) <= uint64(len(buffer)):
			return buffer[:n], nil
		case uint64(n) > uint64(^uint(0)>>1):
			return nil, errors.New("mistlib response size exceeds the platform limit")
		}
		buffer = make([]byte, int(n))
	}
}

func readDLLString(proc *windows.LazyProc) (string, error) {
	data, err := readDLLBytes(0, func(buffer []byte) uint32 {
		n, _, _ := proc.Call(uintptr(dllBytes(buffer)), uintptr(len(buffer)))
		return uint32(n)
	})
	if err != nil {
		return "", fmt.Errorf("mistlib %s: %w", proc.Name, err)
	}
	return strings.ToValidUTF8(string(data), "\uFFFD"), nil
}
