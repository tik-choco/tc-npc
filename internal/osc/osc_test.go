package osc

import (
	"bytes"
	"encoding/hex"
	"strings"
	"testing"
	"unicode/utf8"
)

func TestTruncatesToCharCountNotByteCount(t *testing.T) {
	if got := truncateChars(strings.Repeat("あ", 200), chatboxMaxChars); utf8.RuneCountInString(got) != chatboxMaxChars || !utf8.ValidString(got) {
		t.Fatalf("invalid truncated text %q", got)
	}
}

func TestEncodeMessageKnownBytes(t *testing.T) {
	for _, tt := range []struct {
		name string
		addr string
		args []any
		hex  string
	}{
		{"chatbox", "/chatbox/input", []any{"あ", true, false},
			"2f63686174626f782f696e7075740000" + "2c73544600000000" + "e3818200"},
		{"mixed", "/test", []any{int32(-42), float32(1.5), "abcd", false, true, []byte{0, 1, 2}},
			"2f74657374000000" + "2c69667346546200" + "ffffffd6" + "3fc00000" + "6162636400000000" + "0000000300010200"},
		{"empty", "/test", nil, "2f746573740000002c000000"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			want, err := hex.DecodeString(tt.hex)
			if err != nil {
				t.Fatal(err)
			}
			got, err := EncodeMessage(tt.addr, tt.args...)
			if err != nil || !bytes.Equal(got, want) {
				t.Fatalf("got % x, %v; want % x", got, err, want)
			}
		})
	}
}
