package action

import (
	"context"
	"fmt"
	"math"
	"sync"

	"github.com/tik-choco/tc-npc/internal/bus"
)

const (
	turnDPSSlope     = 400.0
	turnDPSOffset    = 0.5
	minTurnIntensity = float32(0.6)
	moveMPSSlope     = 2.25
	moveMPSOffset    = 0.1
	minMoveIntensity = float32(0.25)
	msgPosition      = "position"
)

func intensityForDPS(targetDPS float64) float32 {
	return min(max(float32(targetDPS/turnDPSSlope+turnDPSOffset), minTurnIntensity), maxAxisValue)
}

func intensityForMPS(targetMPS float64) float32 {
	return min(max(float32(targetMPS/moveMPSSlope+moveMPSOffset), minMoveIntensity), maxAxisValue)
}

func maxMPS() float64 { return moveMPSSlope * (float64(maxAxisValue) - moveMPSOffset) }
func maxDPS() float64 { return turnDPSSlope * (float64(maxAxisValue) - turnDPSOffset) }

type positionData struct {
	X       float64 `json:"x"`
	Y       float64 `json:"y"`
	Heading float64 `json:"heading"`
}

func (p positionData) String() string {
	return fmt.Sprintf("X=%.2f Y=%.2f Heading=%.1f°", p.X, p.Y, p.Heading)
}

type position struct {
	mu   sync.Mutex
	data positionData
}

func (p *position) reset() {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.data = positionData{}
}

// Heading 0 points along +Y and heading 90 points along +X.
func (p *position) addMove(meters float64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	radians := p.data.Heading * math.Pi / 180
	p.data.X += meters * math.Sin(radians)
	p.data.Y += meters * math.Cos(radians)
}

func (p *position) addRotation(degrees float64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	heading := math.Mod(p.data.Heading+degrees, 360)
	if heading < 0 {
		heading += 360
	}
	p.data.Heading = heading
}

func (p *position) get() positionData {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.data
}

type navigator struct {
	body Actuator
	pos  position
	bus  *bus.Bus
}

func (n *navigator) publishPosition() {
	n.bus.Publish(bus.TopicUI, msgPosition, n.pos.get())
}

// A cancelled move contributes only the fraction of the intended hold that
// elapsed. A zero-length hold applies its full delta.
func (n *navigator) trackHold(ctx context.Context, set func(float32) error, update func(float64), delta, seconds float64, intensity float32) (bool, error) {
	duration := secs(seconds)
	if delta < 0 {
		intensity = -intensity
	}
	completed, elapsed, err := holdAxis(ctx, set, intensity, duration)
	if err != nil {
		return false, err
	}
	fraction := 1.0
	if duration != 0 && !completed {
		fraction = min(max(elapsed.Seconds()/duration.Seconds(), 0), 1)
	}
	update(delta * fraction)
	n.publishPosition()
	return completed, nil
}

func (n *navigator) moveMeters(ctx context.Context, meters float64) (bool, error) {
	return n.trackHold(ctx, n.body.Vertical, n.pos.addMove, meters, math.Abs(meters)/maxMPS(), maxAxisValue)
}

func (n *navigator) turnDegrees(ctx context.Context, degrees float64) (bool, error) {
	return n.trackHold(ctx, n.body.LookHorizontal, n.pos.addRotation, degrees, math.Abs(degrees)/maxDPS(), maxAxisValue)
}

func (n *navigator) moveMetersInTime(ctx context.Context, meters, seconds float64) (bool, error) {
	if seconds <= 0 {
		return true, nil
	}
	distance := math.Abs(meters)
	intensity := intensityForMPS(distance / seconds)
	actualMPS := moveMPSSlope * (float64(intensity) - moveMPSOffset)
	return n.trackHold(ctx, n.body.Vertical, n.pos.addMove, meters, distance/actualMPS, intensity)
}

func (n *navigator) turnDegreesInTime(ctx context.Context, degrees, seconds float64) (bool, error) {
	if seconds <= 0 {
		return true, nil
	}
	angle := math.Abs(degrees)
	intensity := intensityForDPS(angle / seconds)
	actualDPS := turnDPSSlope * (float64(intensity) - turnDPSOffset)
	return n.trackHold(ctx, n.body.LookHorizontal, n.pos.addRotation, degrees, angle/actualDPS, intensity)
}
