package speech

import (
	"context"
	"errors"
	"log/slog"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
)

const (
	playbackPollInterval = 15 * time.Millisecond
	levelPublishInterval = 50 * time.Millisecond
)

type playbackClip struct {
	wav      []byte
	priority bool
}

// speakingGate identifies the audible clip for echo suppression and drives
// the avatar from actual host playback. The browser never receives the audio.
type speakingGate struct {
	active atomic.Pointer[playbackClip]
	bus    *bus.Bus
}

func (g *speakingGate) set(clip *playbackClip) {
	active := clip != nil
	if previous := g.active.Swap(clip); (previous != nil) == active {
		return
	}
	g.bus.Publish(bus.TopicUI, UIMsgSpeaking, map[string]bool{"active": active})
	if !active {
		publishLevel(g.bus, UIMsgSpeakingLevel, 0)
	}
}

type playbackWorker struct {
	device string
	cancel context.CancelFunc
	done   chan struct{}
	wake   chan struct{}
	gate   *speakingGate

	mu         sync.Mutex
	queue      []*playbackClip
	cancelClip context.CancelFunc
	closed     bool
}

func startPlayback(ctx context.Context, name string, gate *speakingGate, open func(string) (audio.Playback, error)) *playbackWorker {
	ctx, cancel := context.WithCancel(ctx)
	p := &playbackWorker{
		device: name, cancel: cancel, done: make(chan struct{}), wake: make(chan struct{}, 1), gate: gate,
	}
	go p.run(ctx, open)
	return p
}

func (p *playbackWorker) close() {
	p.cancel()
	<-p.done
}

func (p *playbackWorker) enqueue(items [][]byte, priority bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return
	}
	if priority {
		p.cutLocked()
	}
	for _, wav := range items {
		p.queue = append(p.queue, &playbackClip{wav: wav, priority: priority})
	}
	select {
	case p.wake <- struct{}{}:
	default:
	}
}

// An interrupt from the mic applies only to the clip that was heard. A
// priority announcement may have started by the time the packet is processed.
func (p *playbackWorker) interrupt(heard *playbackClip) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if heard != nil && (heard.priority || p.gate.active.Load() != heard || p.cancelClip == nil) {
		return false
	}
	p.cutLocked()
	return true
}

func (p *playbackWorker) cutLocked() {
	p.queue = nil
	if p.cancelClip != nil {
		p.cancelClip()
		p.cancelClip = nil
	}
}

func (p *playbackWorker) run(ctx context.Context, open func(string) (audio.Playback, error)) {
	defer close(p.done)
	defer func() {
		p.mu.Lock()
		p.closed = true
		p.cutLocked()
		p.mu.Unlock()
	}()
	out, err := open(p.device)
	if err != nil {
		slog.Error("npc-speech: could not open playback device", "error", err)
		return
	}
	defer func() {
		if err := out.Close(); err != nil {
			slog.Error("npc-speech: could not close playback device", "error", err)
		}
	}()
	for ctx.Err() == nil {
		p.mu.Lock()
		if len(p.queue) == 0 {
			p.mu.Unlock()
			select {
			case <-ctx.Done():
				return
			case <-p.wake:
				continue
			}
		}
		clip := p.queue[0]
		p.queue[0] = nil
		p.queue = p.queue[1:]
		clipCtx, cancel := context.WithCancel(ctx)
		p.cancelClip = cancel
		p.mu.Unlock()

		p.play(clipCtx, out, clip)
		cancel()
		p.mu.Lock()
		p.cancelClip = nil
		p.mu.Unlock()
	}
}

func (p *playbackWorker) play(ctx context.Context, out audio.Playback, clip *playbackClip) {
	samples, rate, channels, err := audio.DecodeWAV(clip.wav)
	if err != nil {
		slog.Error("npc-speech: failed to decode tts wav", "error", err)
		return
	}
	if channels > 1 {
		samples = audio.DownmixToMono(samples, channels)
	}
	if rate != out.SampleRate() {
		samples = audio.ResampleLinear(samples, rate, out.SampleRate())
	}
	if len(samples) == 0 || ctx.Err() != nil {
		return
	}
	interleaved := samples
	if channels := out.Channels(); channels > 1 {
		interleaved = make([]float32, len(samples)*channels)
		for i, sample := range samples {
			for ch := range channels {
				interleaved[i*channels+ch] = sample
			}
		}
	}
	start := out.Position()
	p.gate.set(clip)
	defer p.gate.set(nil)
	result := make(chan error, 1)
	go func() {
		err := out.Write(ctx, interleaved)
		if err == nil {
			err = out.Drain(ctx)
		}
		result <- err
	}()
	tick := time.NewTicker(playbackPollInterval)
	defer tick.Stop()
	var last time.Time
	var levelPos int
	for {
		select {
		case <-ctx.Done():
			// Cancellation can land between Write and Drain. Flush explicitly
			// and wait for the old clip before allowing a new one to write.
			out.Flush()
			<-result
			return
		case err := <-result:
			if ctx.Err() != nil {
				out.Flush()
			} else if err != nil && !errors.Is(err, audio.ErrFlushed) {
				slog.Error("npc-speech: playback failed", "error", err)
			}
			return
		case now := <-tick.C:
			position := out.Position()
			if position >= start {
				pos := int(min(position-start, uint64(len(samples))))
				if pos > levelPos && now.Sub(last) >= levelPublishInterval {
					publishLevel(p.gate.bus, UIMsgSpeakingLevel, audio.PlaybackLevel(audio.RMSF32(samples[levelPos:pos])))
					last = now
				}
				levelPos = pos
			}
		}
	}
}
