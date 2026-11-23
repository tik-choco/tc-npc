package action

import (
	"context"
	"errors"
	"math"
	"time"
)

const jumpPressDuration = 100 * time.Millisecond

func secs(seconds float64) time.Duration {
	if seconds <= 0 || math.IsNaN(seconds) {
		return 0
	}
	nanos := math.RoundToEven(seconds * float64(time.Second))
	if nanos >= float64(math.MaxInt64) {
		return time.Duration(math.MaxInt64)
	}
	return time.Duration(nanos)
}

// sleepCancellable reports the time spent holding the input so the navigator
// can prorate a move or turn interrupted by cancellation.
func sleepCancellable(ctx context.Context, duration time.Duration) (completed bool, elapsed time.Duration) {
	if duration == 0 {
		return true, 0
	}
	start := time.Now()
	timer := time.NewTimer(duration)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false, min(time.Since(start), duration)
	case <-timer.C:
		return true, duration
	}
}

// holdAxis releases the axis even when cancellation cuts the hold short.
func holdAxis(ctx context.Context, set func(float32) error, value float32, duration time.Duration) (bool, time.Duration, error) {
	if err := set(value); err != nil {
		return false, 0, err
	}
	completed, elapsed := sleepCancellable(ctx, duration)
	return completed, elapsed, set(0)
}

type controller struct {
	body Actuator
}

func (c controller) moveForward(ctx context.Context, seconds float64) (bool, error) {
	completed, _, err := holdAxis(ctx, c.body.Vertical, maxAxisValue, secs(seconds))
	return completed, err
}

func (c controller) moveBackward(ctx context.Context, seconds float64) (bool, error) {
	completed, _, err := holdAxis(ctx, c.body.Vertical, minAxisValue, secs(seconds))
	return completed, err
}

func (c controller) turn(ctx context.Context, intensity float32, seconds float64) (bool, error) {
	completed, _, err := holdAxis(ctx, c.body.LookHorizontal, intensity, secs(seconds))
	return completed, err
}

func (c controller) moveAndTurn(ctx context.Context, vertical, horizontal, turn float32, seconds float64) (completed bool, err error) {
	axes := [...]struct {
		set   func(float32) error
		value float32
	}{
		{c.body.Vertical, vertical},
		{c.body.Horizontal, horizontal},
		{c.body.LookHorizontal, turn},
	}
	started := 0
	defer func() {
		// Release every axis that was set, even if another axis failed.
		for _, axis := range axes[:started] {
			if releaseErr := axis.set(0); releaseErr != nil {
				err = errors.Join(err, releaseErr)
			}
		}
	}()
	for _, axis := range axes {
		if err := axis.set(axis.value); err != nil {
			return false, err
		}
		started++
	}
	completed, _ = sleepCancellable(ctx, secs(seconds))
	return completed, nil
}

func (c controller) jump(ctx context.Context) (bool, error) {
	if err := c.body.Jump(true); err != nil {
		return false, err
	}
	completed, _ := sleepCancellable(ctx, jumpPressDuration)
	return completed, c.body.Jump(false)
}
