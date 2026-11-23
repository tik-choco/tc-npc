package action

import (
	"context"
	"fmt"
	"log/slog"
	"net"
	"net/netip"
	"strconv"

	"github.com/tik-choco/tc-npc/internal/osc"
)

const defaultOSCAddress = "127.0.0.1:9000"

// VrcClient sends VRChat inputs over an ephemeral UDP socket.
type VrcClient struct {
	socket *net.UDPConn
	target netip.AddrPort
}

var _ Actuator = (*VrcClient)(nil)

func connectVRC(ctx context.Context, address string) (*VrcClient, error) {
	target, err := resolveOSCAddress(ctx, address)
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}
	if err != nil {
		slog.Warn("npc-action: could not resolve osc_address, using default", "addr", address, "fallback", defaultOSCAddress)
		target = netip.MustParseAddrPort(defaultOSCAddress)
	}
	socket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4zero})
	if err != nil {
		return nil, fmt.Errorf("bind OSC socket: %w", err)
	}
	return &VrcClient{socket: socket, target: target}, nil
}

func resolveOSCAddress(ctx context.Context, address string) (netip.AddrPort, error) {
	if target, err := netip.ParseAddrPort(address); err == nil {
		return target, nil
	}
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return netip.AddrPort{}, err
	}
	portNumber, err := strconv.ParseUint(port, 10, 16)
	if err != nil {
		return netip.AddrPort{}, err
	}
	addresses, err := net.DefaultResolver.LookupNetIP(ctx, "ip", host)
	if err != nil {
		return netip.AddrPort{}, err
	}
	if len(addresses) == 0 {
		return netip.AddrPort{}, fmt.Errorf("no IP address for %q", host)
	}
	return netip.AddrPortFrom(addresses[0], uint16(portNumber)), nil
}

func (v *VrcClient) send(address string, arg any) error {
	packet, err := osc.EncodeMessage(address, arg)
	if err != nil {
		return fmt.Errorf("encode OSC message %s: %w", address, err)
	}
	if _, err := v.socket.WriteToUDPAddrPort(packet, v.target); err != nil {
		return fmt.Errorf("send OSC message %s: %w", address, err)
	}
	return nil
}

func (v *VrcClient) sendAxis(address string, value float32) error {
	clamped := min(max(value, minAxisValue), maxAxisValue)
	if value < minAxisValue || value > maxAxisValue {
		slog.Warn("npc-action: OSC axis value clamped", "value", value, "clamped_to", clamped)
	}
	return v.send(address, clamped)
}

// Vertical sets forward or backward movement.
func (v *VrcClient) Vertical(value float32) error { return v.sendAxis("/input/Vertical", value) }

// Horizontal sets sideways movement.
func (v *VrcClient) Horizontal(value float32) error { return v.sendAxis("/input/Horizontal", value) }

// LookHorizontal sets yaw.
func (v *VrcClient) LookHorizontal(value float32) error {
	return v.sendAxis("/input/LookHorizontal", value)
}

// Jump presses or releases the jump button.
func (v *VrcClient) Jump(pressed bool) error {
	var value int32
	if pressed {
		value = 1
	}
	return v.send("/input/Jump", value)
}
