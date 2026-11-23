// Package osc sends fire-and-forget UDP messages to VRChat.
package osc

import (
	"encoding/binary"
	"fmt"
	"log/slog"
	"math"
	"net"
	"strings"
)

const chatboxMaxChars = 144

// EncodeMessage encodes an OSC 1.0 message. Arguments may be int32,
// float32, string, bool, or []byte.
func EncodeMessage(address string, args ...any) ([]byte, error) {
	if !strings.HasPrefix(address, "/") || strings.ContainsRune(address, 0) {
		return nil, fmt.Errorf("invalid OSC address %q", address)
	}

	tags := make([]byte, 1, len(args)+1)
	tags[0] = ','
	var payload []byte
	for _, arg := range args {
		switch value := arg.(type) {
		case int32:
			tags = append(tags, 'i')
			payload = binary.BigEndian.AppendUint32(payload, uint32(value))
		case float32:
			tags = append(tags, 'f')
			payload = binary.BigEndian.AppendUint32(payload, math.Float32bits(value))
		case string:
			if strings.ContainsRune(value, 0) {
				return nil, fmt.Errorf("OSC string contains a NUL")
			}
			tags = append(tags, 's')
			payload = appendOSCString(payload, value)
		case bool:
			tag := byte('F')
			if value {
				tag = 'T'
			}
			tags = append(tags, tag)
		case []byte:
			if len(value) > math.MaxInt32 {
				return nil, fmt.Errorf("OSC blob is too large: %d bytes", len(value))
			}
			tags = append(tags, 'b')
			payload = binary.BigEndian.AppendUint32(payload, uint32(len(value)))
			payload = append(payload, value...)
			payload = append(payload, make([]byte, (4-len(value)%4)%4)...)
		default:
			return nil, fmt.Errorf("unsupported OSC argument type %T", value)
		}
	}

	packet := appendOSCString(nil, address)
	packet = appendOSCString(packet, string(tags))
	return append(packet, payload...), nil
}

func appendOSCString(dst []byte, value string) []byte {
	dst = append(dst, value...)
	return append(dst, make([]byte, 4-len(value)%4)...)
}

// SendChatbox sends up to 144 characters, immediately and without a
// keyboard notification sound. Failures are logged.
func SendChatbox(oscAddress, text string) {
	packet, err := EncodeMessage("/chatbox/input", truncateChars(text, chatboxMaxChars), true, false)
	if err != nil {
		slog.Warn("osc: failed to encode vrc chatbox message", "error", err)
		return
	}

	socket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero})
	if err != nil {
		slog.Warn("osc: failed to bind socket", "error", err)
		return
	}
	defer socket.Close()

	target, err := net.ResolveUDPAddr("udp", oscAddress)
	if err == nil {
		_, err = socket.WriteToUDP(packet, target)
	}
	if err != nil {
		slog.Warn("osc: failed to send vrc chatbox message", "error", err, "target", oscAddress)
	}
}

func truncateChars(s string, max int) string {
	for i := range s {
		if max == 0 {
			return s[:i]
		}
		max--
	}
	return s
}
