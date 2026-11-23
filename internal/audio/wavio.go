package audio

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"

	mp3 "github.com/hajimehoshi/go-mp3"
)

const (
	wavePCM        = 1
	waveFloat      = 3
	waveExtensible = 0xfffe
)

// The final 14 bytes shared by the PCM and IEEE float subformat GUIDs.
const waveSubtypeSuffix = "\x00\x00\x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71"

// EncodeWAVI16 encodes mono PCM16 in the same 44-byte-header WAV as hound.
func EncodeWAVI16(samples []int16, sampleRate int) ([]byte, error) {
	if sampleRate < 0 || uint64(sampleRate)*2 > math.MaxUint32 {
		return nil, fmt.Errorf("invalid WAV sample rate %d", sampleRate)
	}
	if uint64(len(samples)) > (min(uint64(math.MaxInt)-44, math.MaxUint32-36))/2 {
		return nil, errors.New("PCM16 samples exceed the WAV size limit")
	}
	wav := make([]byte, 44+2*len(samples))
	le := binary.LittleEndian
	copy(wav, "RIFF")
	le.PutUint32(wav[4:], uint32(len(wav)-8))
	copy(wav[8:], "WAVEfmt ")
	le.PutUint32(wav[16:], 16)
	le.PutUint16(wav[20:], wavePCM)
	le.PutUint16(wav[22:], 1)
	le.PutUint32(wav[24:], uint32(sampleRate))
	le.PutUint32(wav[28:], uint32(sampleRate)*2)
	le.PutUint16(wav[32:], 2)
	le.PutUint16(wav[34:], 16)
	copy(wav[36:], "data")
	le.PutUint32(wav[40:], uint32(len(wav)-44))
	for i, sample := range samples {
		le.PutUint16(wav[44+2*i:], uint16(sample))
	}
	return wav, nil
}

type waveFormat struct {
	tag, rate, channels, bits, sampleBytes int
}

// DecodeWAV decodes PCM8/16/24/32 or IEEE float32 WAV to interleaved float
// samples and returns the source sample rate and channel count.
func DecodeWAV(data []byte) (samples []float32, sampleRate, channels int, err error) {
	if len(data) < 12 || string(data[:4]) != "RIFF" || string(data[8:12]) != "WAVE" {
		return nil, 0, 0, errors.New("invalid RIFF/WAVE header")
	}
	var format waveFormat
	for chunk := data[12:]; len(chunk) >= 8; {
		id, size := string(chunk[:4]), uint64(binary.LittleEndian.Uint32(chunk[4:]))
		payload := chunk[8:]
		if id == "data" {
			if format.channels == 0 {
				return nil, 0, 0, errors.New("WAV data precedes fmt chunk")
			}
			samples, err := decodeWAVSamples(format, payload[:min(uint64(len(payload)), size)], size)
			if err != nil {
				return nil, 0, 0, err
			}
			return samples, format.rate, format.channels, nil
		}
		if size > uint64(len(payload)) {
			return nil, 0, 0, fmt.Errorf("read WAV %q chunk: %w", id, io.ErrUnexpectedEOF)
		}
		if id == "fmt " {
			format, err = parseWAVFormat(payload[:size])
			if err != nil {
				return nil, 0, 0, err
			}
		}
		chunk = payload[min(size+size%2, uint64(len(payload))):]
	}
	return nil, 0, 0, errors.New("WAV is missing a data chunk")
}

func parseWAVFormat(data []byte) (waveFormat, error) {
	if len(data) < 16 {
		return waveFormat{}, errors.New("short WAV fmt chunk")
	}
	le := binary.LittleEndian
	f := waveFormat{
		tag:      int(le.Uint16(data)),
		rate:     int(le.Uint32(data[4:])),
		channels: int(le.Uint16(data[2:])),
		bits:     int(le.Uint16(data[14:])),
	}
	align := int(le.Uint16(data[12:]))
	if f.channels == 0 || align%f.channels != 0 || f.rate < 0 ||
		uint64(align)*uint64(f.rate) != uint64(le.Uint32(data[8:])) {
		return waveFormat{}, errors.New("inconsistent WAV fmt chunk")
	}
	f.sampleBytes = align / f.channels
	if f.bits == 0 || f.bits%8 != 0 || f.bits > f.sampleBytes*8 {
		return waveFormat{}, errors.New("invalid WAV container bit depth")
	}
	if f.tag == waveExtensible {
		if len(data) < 40 || le.Uint16(data[16:]) < 22 || string(data[26:40]) != waveSubtypeSuffix {
			return waveFormat{}, errors.New("unsupported WAVEFORMATEXTENSIBLE subformat")
		}
		f.tag = int(le.Uint16(data[24:]))
		if valid := le.Uint16(data[18:]); valid != 0 {
			f.bits = int(valid)
		}
	}
	switch f.tag {
	case wavePCM:
		switch f.bits {
		case 8, 16, 24, 32:
			return f, nil
		}
	case waveFloat:
		if f.bits == 32 {
			return f, nil
		}
	}
	return waveFormat{}, fmt.Errorf("unsupported WAV format %d with %d-bit samples", f.tag, f.bits)
}

func decodeWAVSamples(f waveFormat, data []byte, size uint64) ([]float32, error) {
	width := uint64(f.sampleBytes)
	if size%(width*uint64(f.channels)) != 0 {
		return nil, errors.New("WAV data chunk does not contain a whole number of frames")
	}
	count := size / width
	// The Rust decoder substitutes zero for unreadable samples. Bound the
	// amount of missing data a malformed header can cause us to allocate.
	if count > uint64(math.MaxInt)/4 || count-uint64(len(data))/width > 1<<20 {
		return nil, fmt.Errorf("read WAV samples: %w", io.ErrUnexpectedEOF)
	}
	samples := make([]float32, int(count))
	if f.sampleBytes != f.bits/8 && !(f.tag == wavePCM && f.bits == 24 && f.sampleBytes == 4) {
		return samples, nil // hound yields a per-sample error for this layout.
	}
	le := binary.LittleEndian
	for i := range len(data) / f.sampleBytes {
		b := data[i*f.sampleBytes:]
		switch {
		case f.tag == waveFloat:
			samples[i] = math.Float32frombits(le.Uint32(b))
		case f.bits == 8:
			samples[i] = float32(int(b[0])-128) / 128
		case f.bits == 16:
			samples[i] = float32(int16(le.Uint16(b))) / 32768
		case f.bits == 24:
			// Hound also reads 24-bit samples in a four-byte container
			// as signed, low-aligned 24-bit integers.
			n := int32(b[0]) | int32(b[1])<<8 | int32(b[2])<<16
			samples[i] = float32(n<<8>>8) / 8388608
		case f.bits == 32:
			samples[i] = float32(int32(le.Uint32(b))) / 2147483648
		}
	}
	return samples, nil
}

// DecodeAudio decodes a sound-library clip. RIFF magic selects WAV. Other
// inputs go through the MP3 decoder.
func DecodeAudio(data []byte) (samples []float32, sampleRate, channels int, err error) {
	if bytes.HasPrefix(data, []byte("RIFF")) {
		return DecodeWAV(data)
	}
	frames, channels, err := mp3Frames(data)
	if err != nil {
		return nil, 0, 0, err
	}
	// Only a reader is needed. Hide Seek so go-mp3 can decode in one pass.
	decoder, err := mp3.NewDecoder(struct{ io.Reader }{bytes.NewReader(frames)})
	if err != nil {
		return nil, 0, 0, fmt.Errorf("open MP3 decoder: %w", err)
	}
	var pcm [4096]byte
	for {
		n, readErr := io.ReadFull(decoder, pcm[:])
		for i := 0; i+4 <= n; i += 4 {
			samples = append(samples, float32(int16(binary.LittleEndian.Uint16(pcm[i:])))/32768)
			if channels == 2 {
				samples = append(samples, float32(int16(binary.LittleEndian.Uint16(pcm[i+2:])))/32768)
			}
		}
		switch readErr {
		case nil:
			continue
		case io.EOF, io.ErrUnexpectedEOF:
			if len(samples) == 0 {
				return nil, 0, 0, errors.New("MP3 decoded to no samples")
			}
			return samples, decoder.SampleRate(), channels, nil
		default:
			return nil, 0, 0, fmt.Errorf("decode MP3: %w", readErr)
		}
	}
}

// go-mp3 duplicates mono into stereo. Read the channel mode from the first
// frame so mono files retain their source channel count.
func mp3Frames(data []byte) ([]byte, int, error) {
	if bytes.HasPrefix(data, []byte("ID3")) {
		if len(data) < 10 || (data[6]|data[7]|data[8]|data[9])&0x80 != 0 {
			return nil, 0, errors.New("invalid MP3 ID3 header")
		}
		size := int(data[6])<<21 | int(data[7])<<14 | int(data[8])<<7 | int(data[9])
		if 10+size > len(data) {
			return nil, 0, fmt.Errorf("read MP3 ID3 tag: %w", io.ErrUnexpectedEOF)
		}
		data = data[10+size:]
	} else if bytes.HasPrefix(data, []byte("TAG")) && len(data) >= 128 {
		data = data[128:]
	}
	for len(data) >= 4 {
		h := binary.BigEndian.Uint32(data)
		version, layer, bitrate, rate := (h>>19)&3, (h>>17)&3, (h>>12)&15, (h>>10)&3
		if h>>21 == 0x7ff && version != 1 && layer != 0 && bitrate != 15 && rate != 3 && h&3 != 2 {
			channels := 2
			if (h>>6)&3 == 3 {
				channels = 1
			}
			return data, channels, nil
		}
		data = data[1:]
	}
	return nil, 0, errors.New("unsupported audio format: no MP3 frame found")
}
