package audio

import (
	"encoding/binary"
	"math"
	"slices"
	"testing"
)

func TestRoundTripsPCM16(t *testing.T) {
	samples := []int16{0, 1000, -1000, 32767, -32768}
	wav, err := EncodeWAVI16(samples, 16000)
	if err != nil {
		t.Fatal(err)
	}
	decoded, rate, channels, err := DecodeWAV(wav)
	if err != nil {
		t.Fatal(err)
	}
	if rate != 16000 || channels != 1 || len(decoded) != len(samples) {
		t.Fatalf("decoded %d samples at %d Hz with %d channels", len(decoded), rate, channels)
	}
	for i, sample := range samples {
		if decoded[i] != float32(sample)/32768 {
			t.Errorf("sample %d = %v, expected normalized PCM16 %d", i, decoded[i], sample)
		}
	}
}

func TestDecodeAudioRoutesWAVToWAVDecoder(t *testing.T) {
	wav, err := EncodeWAVI16([]int16{0, 1000, -1000}, 22050)
	if err != nil {
		t.Fatal(err)
	}
	viaAudio, rate, channels, err := DecodeAudio(wav)
	if err != nil {
		t.Fatal(err)
	}
	viaWAV, _, _, err := DecodeWAV(wav)
	if err != nil {
		t.Fatal(err)
	}
	if rate != 22050 || channels != 1 || !slices.Equal(viaAudio, viaWAV) {
		t.Fatalf("DecodeAudio = %v at %d Hz with %d channels, DecodeWAV = %v", viaAudio, rate, channels, viaWAV)
	}
}

func TestDecodeAudioErrorsOnUnknownFormat(t *testing.T) {
	for _, input := range [][]byte{[]byte("not audio at all, just some bytes"), nil, []byte("RIFF")} {
		if _, _, _, err := DecodeAudio(input); err == nil {
			t.Errorf("DecodeAudio(%q) succeeded", input)
		}
	}
}

func TestDecodeWAVSampleFormats(t *testing.T) {
	tests := []struct {
		name          string
		tag           uint16
		containerBits uint16
		validBits     uint16 // Nonzero means WAVEFORMATEXTENSIBLE.
		data          []byte
		want          []float32
	}{
		{"PCM8", wavePCM, 8, 0, []byte{0, 192}, []float32{-1, 0.5}},
		{"PCM16", wavePCM, 16, 0, []byte{0, 128, 0, 64}, []float32{-1, 0.5}},
		{"PCM24", wavePCM, 24, 0, []byte{0, 0, 128, 0, 0, 64}, []float32{-1, 0.5}},
		{"PCM32", wavePCM, 32, 0, []byte{0, 0, 0, 128, 0, 0, 0, 64}, []float32{-1, 0.5}},
		{"float32", waveFloat, 32, 0, []byte{0, 0, 192, 191, 0, 0, 0, 63}, []float32{-1.5, 0.5}},
		{"extensible PCM24", wavePCM, 32, 24, []byte{0, 0, 128, 0, 0, 0, 64, 0}, []float32{-1, 0.5}},
		{"extensible float32", waveFloat, 32, 32, []byte{0, 0, 192, 191, 0, 0, 0, 63}, []float32{-1.5, 0.5}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			le := binary.LittleEndian
			const rate, channels = 48000, 2
			format := make([]byte, 16)
			le.PutUint16(format, tt.tag)
			le.PutUint16(format[2:], channels)
			le.PutUint32(format[4:], rate)
			align := tt.containerBits / 8 * channels
			le.PutUint32(format[8:], rate*uint32(align))
			le.PutUint16(format[12:], align)
			le.PutUint16(format[14:], tt.containerBits)
			if tt.validBits != 0 {
				le.PutUint16(format, waveExtensible)
				format = le.AppendUint16(format, 22)
				format = le.AppendUint16(format, tt.validBits)
				format = le.AppendUint32(format, 3)
				format = le.AppendUint16(format, tt.tag)
				format = append(format, waveSubtypeSuffix...)
			}
			wav := []byte("RIFF\x00\x00\x00\x00WAVEJUNK\x01\x00\x00\x00x\x00fmt ")
			wav = le.AppendUint32(wav, uint32(len(format)))
			wav = append(wav, format...)
			wav = append(wav, "data"...)
			wav = le.AppendUint32(wav, uint32(len(tt.data)))
			wav = append(wav, tt.data...)
			le.PutUint32(wav[4:], uint32(len(wav)-8))

			got, gotRate, gotChannels, err := DecodeWAV(wav)
			if err != nil || gotRate != rate || gotChannels != channels || !slices.Equal(got, tt.want) {
				t.Fatalf("DecodeWAV = %v, %d Hz, %d channels, %v; want %v", got, gotRate, gotChannels, err, tt.want)
			}
		})
	}
}

func TestDecodeWAVZeroPadsTruncatedSamples(t *testing.T) {
	wav, err := EncodeWAVI16([]int16{16384, 16384, 16384}, 16000)
	if err != nil {
		t.Fatal(err)
	}
	decoded, _, _, err := DecodeWAV(wav[:len(wav)-3])
	if err != nil || !slices.Equal(decoded, []float32{0.5, 0, 0}) {
		t.Fatalf("DecodeWAV = %v, %v", decoded, err)
	}
	binary.LittleEndian.PutUint32(wav[40:], math.MaxUint32-1)
	if _, _, _, err := DecodeWAV(wav); err == nil {
		t.Fatal("DecodeWAV accepted an oversized missing data chunk")
	}
}

func TestDecodeAudioMP3ChannelModes(t *testing.T) {
	for _, tt := range []struct {
		name     string
		mode     byte
		channels int
	}{
		{"mono", 0xc0, 1},
		{"stereo", 0, 2},
	} {
		t.Run(tt.name, func(t *testing.T) {
			// Empty side info and main data encode silence. This MPEG-1
			// Layer III frame has 1152 samples at 44100 Hz and 128 kb/s.
			frame := make([]byte, 417)
			copy(frame, []byte{0xff, 0xfb, 0x90, tt.mode})
			// A sync-like byte sequence in the ID3 tag is metadata.
			mp3 := append([]byte("ID3\x04\x00\x00\x00\x00\x00\x04\xff\xfb\x90\xc0"), frame...)
			got, rate, channels, err := DecodeAudio(mp3)
			if err != nil || rate != 44100 || channels != tt.channels || len(got) != 1152*tt.channels {
				t.Fatalf("DecodeAudio = %d samples, %d Hz, %d channels, %v", len(got), rate, channels, err)
			}
			if rms := RMSF32(got); rms != 0 {
				t.Fatalf("silence RMS = %g", rms)
			}
		})
	}
}
