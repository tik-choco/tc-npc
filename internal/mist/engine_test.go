package mist

import (
	"encoding/json"
	"testing"
)

func TestSignalingMatchesWebNamespaceAndSupportsWebSocketOverride(t *testing.T) {
	for _, tc := range []struct{ url, wire string }{
		{"", `{"signaling":{"mode":"nostr","nostr":{"relays":[],"discoveryKind":25049,"messageKind":25050,"ttlSeconds":600,"inviteSalt":"tik-choco-v1","inviteCode":"tik-choco-public-v1"}}}`},
		{" wss://example.test/signal ", `{"signalingUrl":"wss://example.test/signal","signaling":{"mode":"websocket","nostr":null}}`},
	} {
		data, err := json.Marshal(makeEngineConfig(tc.url))
		if err != nil || string(data) != tc.wire {
			t.Errorf("signaling config for %q = %s, %v\nwant %s", tc.url, data, err, tc.wire)
		}
	}
}
