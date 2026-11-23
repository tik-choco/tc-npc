package action

import (
	"context"
	"slices"
	"sync"
	"testing"
	"testing/synctest"
	"time"
)

type actuatorCall struct {
	input   string
	value   float32
	pressed bool
}

// RecordingActuator records the inputs in the order the body received them.
type RecordingActuator struct {
	mu    sync.Mutex
	calls []actuatorCall
}

func (a *RecordingActuator) record(call actuatorCall) error {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.calls = append(a.calls, call)
	return nil
}

func (a *RecordingActuator) Calls() []actuatorCall {
	a.mu.Lock()
	defer a.mu.Unlock()
	return slices.Clone(a.calls)
}

func (a *RecordingActuator) Vertical(value float32) error {
	return a.record(actuatorCall{input: "vertical", value: value})
}

func (a *RecordingActuator) Horizontal(value float32) error {
	return a.record(actuatorCall{input: "horizontal", value: value})
}

func (a *RecordingActuator) LookHorizontal(value float32) error {
	return a.record(actuatorCall{input: "look", value: value})
}

func (a *RecordingActuator) Jump(pressed bool) error {
	return a.record(actuatorCall{input: "jump", pressed: pressed})
}

func assertCalls(t *testing.T, body *RecordingActuator, want ...actuatorCall) {
	t.Helper()
	if got := body.Calls(); !slices.Equal(got, want) {
		t.Fatalf("actuator calls = %+v, want %+v", got, want)
	}
}

func TestMoveForwardHoldsAxisThenZeroesIt(t *testing.T) {
	body := &RecordingActuator{}
	controller := controller{body: body}
	completed, err := controller.moveForward(t.Context(), 0)
	if err != nil || !completed {
		t.Fatalf("completed = %t, error = %v", completed, err)
	}
	assertCalls(t, body, actuatorCall{input: "vertical", value: maxAxisValue}, actuatorCall{input: "vertical"})
}

func TestCancelledHoldStillZeroesAxis(t *testing.T) {
	body := &RecordingActuator{}
	controller := controller{body: body}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()

	completed, err := controller.moveForward(ctx, 60)
	if err != nil || completed {
		t.Fatalf("completed = %t, error = %v", completed, err)
	}
	assertCalls(t, body, actuatorCall{input: "vertical", value: maxAxisValue}, actuatorCall{input: "vertical"})
}

func TestTurnReleasesLookAxis(t *testing.T) {
	body := &RecordingActuator{}
	controller := controller{body: body}
	if _, err := controller.turn(t.Context(), 0.75, 0); err != nil {
		t.Fatal(err)
	}
	assertCalls(t, body, actuatorCall{input: "look", value: 0.75}, actuatorCall{input: "look"})
}

func TestJumpPressesThenReleases(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		body := &RecordingActuator{}
		controller := controller{body: body}
		start := time.Now()
		if _, err := controller.jump(t.Context()); err != nil {
			t.Fatal(err)
		}
		if elapsed := time.Since(start); elapsed != 100*time.Millisecond {
			t.Fatalf("jump held for %s", elapsed)
		}
		assertCalls(t, body, actuatorCall{input: "jump", pressed: true}, actuatorCall{input: "jump"})
	})
}
