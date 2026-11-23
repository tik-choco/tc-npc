package audio

import (
	"context"
	"errors"
	"fmt"
	"io"
	"sync"
	"sync/atomic"
	"time"
)

type streamState struct {
	rate, channels int
	stopping       chan struct{}
	done           chan struct{}
	closeOnce      sync.Once
	err            error // Read only after done closes.
}

// open waits until the worker has initialized or has cleaned up after a setup
// failure. The worker owns all the backend resources until it exits.
func (s *streamState) open(run func(ready func(rate, channels int)) error) error {
	s.stopping = make(chan struct{})
	s.done = make(chan struct{})
	ready := make(chan struct{})
	go func() {
		defer close(s.done)
		s.err = run(func(rate, channels int) {
			s.rate, s.channels = rate, channels
			close(ready)
		})
	}()
	select {
	case <-ready:
		return nil
	case <-s.done:
		return s.err
	}
}

func (s *streamState) SampleRate() int { return s.rate }
func (s *streamState) Channels() int   { return s.channels }

func (s *streamState) Close() error {
	s.closeOnce.Do(func() { close(s.stopping) })
	<-s.done
	return s.err
}

type renderDevice interface {
	padding() (int, error)
	queue(samples []float32) error
	start() error
	stop() error
	reset() error
}

type renderRequest struct {
	ctx     context.Context
	samples []float32
	drain   bool
	result  chan error
}

type playbackStream struct {
	streamState
	position atomic.Uint64
	requests chan *renderRequest
	flush    chan chan struct{}
}

func newPlaybackStream() *playbackStream {
	return &playbackStream{
		requests: make(chan *renderRequest),
		flush:    make(chan chan struct{}),
	}
}

func (p *playbackStream) Write(ctx context.Context, interleaved []float32) error {
	if len(interleaved)%p.Channels() != 0 {
		return errors.New("playback samples must contain whole interleaved frames")
	}
	return p.request(ctx, interleaved, false)
}

func (p *playbackStream) Drain(ctx context.Context) error {
	return p.request(ctx, nil, true)
}

func (p *playbackStream) Position() uint64 { return p.position.Load() }

func (p *playbackStream) request(ctx context.Context, samples []float32, drain bool) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	req := &renderRequest{ctx: ctx, samples: samples, drain: drain, result: make(chan error, 1)}
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-p.done:
		return p.closedError()
	case p.requests <- req:
	}
	// Once accepted, wait for the backend to release the caller's slice.
	select {
	case err := <-req.result:
		return err
	case <-p.done:
		select {
		case err := <-req.result:
			return err
		default:
			return p.closedError()
		}
	}
}

func (p *playbackStream) closedError() error {
	if p.err != nil {
		return p.err
	}
	return io.ErrClosedPipe
}

func (p *playbackStream) Flush() {
	flushed := make(chan struct{})
	select {
	case <-p.done:
		return
	case p.flush <- flushed:
	}
	select {
	case <-flushed:
	case <-p.done:
	}
}

func (p *playbackStream) run(device renderDevice, bufferFrames int, poll, latency time.Duration) (err error) {
	ticker := time.NewTicker(poll)
	defer ticker.Stop()
	var (
		active     *renderRequest
		submitted  uint64
		started    bool
		tracking   bool
		drainUntil time.Time
	)
	defer func() {
		if started {
			err = errors.Join(err, device.stop())
		}
	}()
	discard := func() error {
		if started {
			if err := device.stop(); err != nil {
				return err
			}
			started = false
			if err := device.reset(); err != nil {
				return err
			}
		}
		submitted, tracking, drainUntil = 0, false, time.Time{}
		p.position.Store(0)
		return nil
	}
	for {
		var requests <-chan *renderRequest
		var cancelled <-chan struct{}
		var tick <-chan time.Time
		if active == nil {
			requests = p.requests
		} else {
			cancelled = active.ctx.Done()
		}
		if active != nil || tracking {
			tick = ticker.C
		}
		select {
		case <-p.stopping:
			return nil
		case flushed := <-p.flush:
			if err := discard(); err != nil {
				return err
			}
			if active != nil {
				active.result <- ErrFlushed
				active = nil
			}
			close(flushed)
			continue
		case active = <-requests:
			// A request can race with cancellation while being accepted.
			if err := active.ctx.Err(); err != nil {
				active.result <- err
				active = nil
				continue
			}
		case <-cancelled:
		case <-tick:
		}

		if active != nil && active.ctx.Err() != nil {
			if err := discard(); err != nil {
				return err
			}
			active.result <- active.ctx.Err()
			active = nil
			continue
		}

		var padding int
		if started {
			padding, err = device.padding()
			if err != nil {
				return err
			}
			if padding < 0 || padding > bufferFrames || uint64(padding) > submitted {
				return fmt.Errorf("invalid playback padding of %d frames", padding)
			}
			p.position.Store(submitted - uint64(padding))
			if tracking && padding == 0 {
				now := time.Now()
				if drainUntil.IsZero() {
					// The engine has read the buffer. Allow its reported
					// stream latency for the final frames to reach the device.
					drainUntil = now.Add(latency)
				}
				tracking = now.Before(drainUntil)
			}
		}
		if active == nil {
			continue
		}
		if active.drain {
			if tracking {
				continue
			}
		} else if len(active.samples) > 0 {
			frames := min(len(active.samples)/p.Channels(), bufferFrames-padding)
			if frames == 0 {
				continue
			}
			count := frames * p.Channels()
			if err := device.queue(active.samples[:count]); err != nil {
				return err
			}
			submitted += uint64(frames)
			tracking, drainUntil = true, time.Time{}
			if !started {
				if err := device.start(); err != nil {
					return err
				}
				started = true
			}
			active.samples = active.samples[count:]
			if len(active.samples) > 0 {
				continue
			}
		}
		active.result <- nil
		active = nil
	}
}
