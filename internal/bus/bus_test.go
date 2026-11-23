package bus

import (
	"testing"
)

func TestPublishSubscribeRoundtrip(t *testing.T) {
	b := New()
	sub := b.Subscribe()
	defer sub.Close()
	b.Publish(TopicChat, MsgChatResponse, map[string]any{"text": "hi"})
	got := <-sub.C
	if got.Topic != TopicChat || got.Type != MsgChatResponse {
		t.Fatalf("unexpected message: %+v", got)
	}
	if got.Str("text") != "hi" {
		t.Fatalf("payload text = %q", got.Str("text"))
	}
}

func TestPublishNeverBlocksOnFullSubscriber(t *testing.T) {
	b := New()
	sub := b.Subscribe()
	defer sub.Close()
	for i := 0; i < ChannelCapacity+10; i++ {
		b.Publish(TopicUI, "x", nil)
	}
	if sub.Dropped() != 10 {
		t.Fatalf("dropped = %d, want 10", sub.Dropped())
	}
}

func TestSpeechPayloadOmitsEmpty(t *testing.T) {
	p := SpeechPayload("hello", "  ", "", false)
	if len(p) != 1 || p["content"] != "hello" {
		t.Fatalf("unexpected payload: %v", p)
	}
	p = SpeechPayload("hello", " bob ", "r1", true)
	if p["speaker"] != "bob" || p["request_id"] != "r1" || p["transliterate_name"] != true {
		t.Fatalf("unexpected payload: %v", p)
	}
}
