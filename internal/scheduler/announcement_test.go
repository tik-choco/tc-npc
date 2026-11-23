package scheduler

import (
	"encoding/json"
	"reflect"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

func receive(t *testing.T, sub *bus.Subscription, topic, msgType string) bus.Message {
	t.Helper()
	select {
	case msg := <-sub.C:
		if msg.Topic != topic || msg.Type != msgType {
			t.Fatalf("received %s/%s, want %s/%s", msg.Topic, msg.Type, topic, msgType)
		}
		return msg
	default:
		t.Fatalf("no message received, want %s/%s", topic, msgType)
		return bus.Message{}
	}
}

func noMessage(t *testing.T, sub *bus.Subscription) {
	t.Helper()
	select {
	case msg := <-sub.C:
		t.Fatalf("unexpected message: %+v", msg)
	default:
	}
}

func checkPayload(t *testing.T, msg bus.Message, want map[string]any) {
	t.Helper()
	var got map[string]any
	if err := msg.Decode(&got); err != nil || !reflect.DeepEqual(got, want) {
		t.Fatalf("payload = %s, err %v, want %+v", msg.Payload, err, want)
	}
}

func TestFireAnnouncementPublishesTTSInterrupt(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	ann := announcement("09:00")
	ann.ChimeFile = "assets/chime.wav"
	defaults := config.DefaultSchedulerDefaults()
	if got := FireAnnouncement(b, &ann, &defaults); got != (FireOutcome{Spoke: true}) || !got.Fired() {
		t.Fatalf("outcome = %+v, want speech only", got)
	}
	checkPayload(t, receive(t, sub, bus.TopicInterrupt, bus.MsgTTS), map[string]any{
		"content": "hello", "chime_file": "assets/chime.wav", "volume": 1.0,
		"bgm_file": "", "bgm_volume": 0.5, "bgm_play_full": false, "bgm_end_time": "",
	})
	noMessage(t, sub)
}

func TestFireAnnouncementPublishesBGMFields(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	ann := announcement("09:00")
	ann.BGMFile = "assets/bgm.mp3"
	ann.BGMVolume = 0.5
	ann.BGMPlayFull = true
	ann.BGMEndTime = "1:30"
	defaults := config.DefaultSchedulerDefaults()
	defaults.BGMVolume = 0.25
	if got := FireAnnouncement(b, &ann, &defaults); !got.Spoke {
		t.Fatalf("outcome = %+v, want speech", got)
	}
	checkPayload(t, receive(t, sub, bus.TopicInterrupt, bus.MsgTTS), map[string]any{
		"content": "hello", "chime_file": "", "volume": 1.0,
		"bgm_file": "assets/bgm.mp3", "bgm_volume": 0.5, "bgm_play_full": true, "bgm_end_time": "1:30",
	})
}

func TestFireAnnouncementFallsBackToSchedulerDefaults(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	ann := announcement("09:00")
	ann.Volume = 0
	defaults := config.SchedulerDefaults{ChimeFile: "assets/default_chime.wav", Volume: 0.8, BGMVolume: 0.3}
	FireAnnouncement(b, &ann, &defaults)
	checkPayload(t, receive(t, sub, bus.TopicInterrupt, bus.MsgTTS), map[string]any{
		"content": "hello", "chime_file": defaults.ChimeFile, "volume": float64(defaults.Volume),
		"bgm_file": "", "bgm_volume": float64(defaults.BGMVolume), "bgm_play_full": false, "bgm_end_time": "",
	})
}

func TestFireAnnouncementSkipsEmptyText(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	ann := announcement("09:00")
	ann.Text = ""
	defaults := config.DefaultSchedulerDefaults()
	if got := FireAnnouncement(b, &ann, &defaults); got.Fired() {
		t.Fatalf("outcome = %+v, want no-op", got)
	}
	noMessage(t, sub)
}

func TestFireAnnouncementRunsActionsWithoutText(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	ann := announcement("17:00")
	ann.Text = ""
	ann.Actions = []config.ScheduledAction{{Kind: config.ScheduledActionAction, Content: "原点に移動して"}}
	defaults := config.DefaultSchedulerDefaults()
	if got := FireAnnouncement(b, &ann, &defaults); got != (FireOutcome{Actions: 1}) || !got.Fired() {
		t.Fatalf("outcome = %+v, want one action", got)
	}
	checkPayload(t, receive(t, sub, bus.TopicAction, bus.MsgAction), map[string]any{"content": "原点に移動して"})
	noMessage(t, sub)
}

func TestFireAnnouncementSpeaksThenRunsActionsInOrder(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	ann := announcement("17:00")
	ann.Actions = []config.ScheduledAction{
		{Kind: config.ScheduledActionCommand, Text: "route patrol"},
		{Kind: config.ScheduledActionResume},
	}
	defaults := config.DefaultSchedulerDefaults()
	if got := FireAnnouncement(b, &ann, &defaults); got != (FireOutcome{Spoke: true, Actions: 2}) {
		t.Fatalf("outcome = %+v, want speech and two actions", got)
	}
	receive(t, sub, bus.TopicInterrupt, bus.MsgTTS)
	checkPayload(t, receive(t, sub, bus.TopicAction, "command"), map[string]any{"text": "route patrol"})
	checkPayload(t, receive(t, sub, bus.TopicInterrupt, bus.MsgResume), map[string]any{})
	noMessage(t, sub)
}

func TestRawActionPublishesVerbatim(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	action := config.ScheduledAction{
		Kind: config.ScheduledActionRaw, Topic: "agent:interrupt", MessageType: "resume",
		Payload: json.RawMessage(`{ "why": "test" }`),
	}
	if !ExecuteAction(b, &action) {
		t.Fatal("raw action did not publish")
	}
	msg := receive(t, sub, "agent:interrupt", "resume")
	if string(msg.Payload) != string(action.Payload) {
		t.Fatalf("payload = %s, want %s", msg.Payload, action.Payload)
	}
}

func TestBlankActionsPublishNothing(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	for _, action := range []config.ScheduledAction{
		{Kind: config.ScheduledActionAction, Content: "  "},
		{Kind: config.ScheduledActionCommand},
		{Kind: config.ScheduledActionChat},
		{Kind: config.ScheduledActionSpeak},
		{Kind: config.ScheduledActionRaw, MessageType: "resume", Payload: json.RawMessage("{}")},
		{Kind: config.ScheduledActionRaw, Topic: "agent:interrupt", MessageType: " "},
	} {
		if ExecuteAction(b, &action) {
			t.Errorf("unexpected publication for %+v", action)
		}
	}
	noMessage(t, sub)
}

func TestChatActionInjectsSpeechSenseMessage(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	action := config.ScheduledAction{Kind: config.ScheduledActionChat, Content: "今日の予定は?"}
	if !ExecuteAction(b, &action) {
		t.Fatal("chat action did not publish")
	}
	checkPayload(t, receive(t, sub, bus.TopicSense, bus.MsgSpeech), map[string]any{"content": "今日の予定は?"})
}

func TestSpeakWithOnlyChimeAndSuspend(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	speak := config.ScheduledAction{Kind: config.ScheduledActionSpeak, ChimeFile: "chime.wav"}
	if !ExecuteAction(b, &speak) {
		t.Fatal("chime-only speak action did not publish")
	}
	checkPayload(t, receive(t, sub, bus.TopicInterrupt, bus.MsgTTS), map[string]any{"content": "", "chime_file": "chime.wav"})
	suspend := config.ScheduledAction{Kind: config.ScheduledActionSuspend}
	if !ExecuteAction(b, &suspend) {
		t.Fatal("suspend action did not publish")
	}
	checkPayload(t, receive(t, sub, bus.TopicInterrupt, bus.MsgSuspend), map[string]any{})
}
