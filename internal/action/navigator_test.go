package action

import (
	"context"
	"math"
	"testing"
	"testing/synctest"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
)

func TestIntensitySolvesLinearModel(t *testing.T) {
	for _, tt := range []struct {
		name   string
		solver func(float64) float32
		speed  float64
		want   float32
	}{
		{"maximum turn", intensityForDPS, 200, 1},
		{"half turn speed", intensityForDPS, 100, 0.75},
		{"minimum turn", intensityForDPS, 0, 0.6},
		{"clamped turn", intensityForDPS, 10000, 1},
		{"two meters per second", intensityForMPS, 2, 2.0/2.25 + 0.1},
		{"minimum move", intensityForMPS, 0, 0.25},
		{"clamped move", intensityForMPS, 10000, 1},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := tt.solver(tt.speed); !(math.Abs(float64(got-tt.want)) < 1e-6) {
				t.Fatalf("intensity = %v, want %v", got, tt.want)
			}
		})
	}
}

func near(t *testing.T, got, want float64) {
	t.Helper()
	if !(math.Abs(got-want) < 1e-9) {
		t.Fatalf("value = %.12g, want %.12g", got, want)
	}
}

func TestMaxSpeedsMatchHandComputation(t *testing.T) {
	near(t, maxDPS(), 200)
	near(t, maxMPS(), 2.025)
}

func TestDeadReckoningAddsMoveAlongHeading(t *testing.T) {
	var pos position
	pos.addMove(10)
	p := pos.get()
	near(t, p.X, 0)
	near(t, p.Y, 10)

	pos.addRotation(90)
	pos.addMove(5)
	p = pos.get()
	near(t, p.X, 5)
	near(t, p.Y, 10)
	near(t, p.Heading, 90)
}

func TestDeadReckoningWrapsRotation(t *testing.T) {
	var pos position
	pos.addRotation(370)
	near(t, pos.get().Heading, 10)
	pos.reset()
	pos.addRotation(90)
	pos.addRotation(-100)
	near(t, pos.get().Heading, 350)
}

func TestResetClearsPosition(t *testing.T) {
	var pos position
	pos.addMove(5)
	pos.addRotation(45)
	pos.reset()
	if got := pos.get(); got != (positionData{}) {
		t.Fatalf("position = %+v, want origin", got)
	}
}

func TestCancelledMovePublishesPartialPosition(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		body := &RecordingActuator{}
		b := bus.New()
		nav := navigator{body: body, bus: b}
		sub := b.Subscribe()
		defer sub.Close()

		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		go func() {
			time.Sleep(250 * time.Millisecond)
			cancel()
		}()

		completed, err := nav.moveMeters(ctx, maxMPS())
		if err != nil || completed {
			t.Fatalf("completed = %t, error = %v", completed, err)
		}
		assertCalls(t, body, actuatorCall{input: "vertical", value: maxAxisValue}, actuatorCall{input: "vertical"})
		select {
		case msg := <-sub.C:
			if msg.Topic != bus.TopicUI || msg.Type != msgPosition {
				t.Fatalf("unexpected frame: %+v", msg)
			}
			var p positionData
			if err := msg.Decode(&p); err != nil {
				t.Fatal(err)
			}
			near(t, p.X, 0)
			near(t, p.Y, maxMPS()/4)
			near(t, p.Heading, 0)
		default:
			t.Fatal("no position frame")
		}
	})
}
