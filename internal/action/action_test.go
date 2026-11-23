package action

import (
	"bytes"
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
	"github.com/tik-choco/tc-npc/internal/osc"
)

const testTimeout = 5 * time.Second

func listenOSC(t *testing.T) *net.UDPConn {
	t.Helper()
	socket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = socket.Close() })
	return socket
}

func expectOSC(t *testing.T, socket *net.UDPConn, address string, arg any) {
	t.Helper()
	want, err := osc.EncodeMessage(address, arg)
	if err != nil {
		t.Fatal(err)
	}
	if err := socket.SetReadDeadline(time.Now().Add(testTimeout)); err != nil {
		t.Fatal(err)
	}
	buf := make([]byte, 256)
	n, _, err := socket.ReadFromUDP(buf)
	if err != nil {
		t.Fatalf("read OSC packet for %s: %v", address, err)
	}
	if !bytes.Equal(buf[:n], want) {
		t.Fatalf("OSC packet = % x, want % x (%s %v)", buf[:n], want, address, arg)
	}
}

func awaitUI(t *testing.T, sub *bus.Subscription, kind string, match func(bus.Message) bool) bus.Message {
	t.Helper()
	timer := time.NewTimer(testTimeout)
	defer timer.Stop()
	for {
		select {
		case msg := <-sub.C:
			if msg.Topic == bus.TopicUI && msg.Type == kind && match(msg) {
				return msg
			}
		case <-timer.C:
			t.Fatalf("no matching %s UI frame", kind)
		}
	}
}

func awaitActionLog(t *testing.T, sub *bus.Subscription, text string) {
	t.Helper()
	awaitUI(t, sub, msgActionLog, func(msg bus.Message) bool { return strings.Contains(msg.Str("text"), text) })
}

func TestVRCClientEncodesAndClampsInputs(t *testing.T) {
	socket := listenOSC(t)
	body, err := connectVRC(t.Context(), socket.LocalAddr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer body.socket.Close()
	for _, tt := range []struct {
		address string
		want    any
		call    func() error
	}{
		{"/input/Vertical", float32(1), func() error { return body.Vertical(1.25) }},
		{"/input/Horizontal", float32(-1), func() error { return body.Horizontal(-2) }},
		{"/input/LookHorizontal", float32(0.75), func() error { return body.LookHorizontal(0.75) }},
		{"/input/Jump", int32(1), func() error { return body.Jump(true) }},
		{"/input/Jump", int32(0), func() error { return body.Jump(false) }},
	} {
		if err := tt.call(); err != nil {
			t.Fatal(err)
		}
		expectOSC(t, socket, tt.address, tt.want)
	}
}

func TestModuleCommandsReloadAndPlan(t *testing.T) {
	socket := listenOSC(t)
	requests := make(chan llm.ChatRequest, 2)
	awaitRequest := func() llm.ChatRequest {
		t.Helper()
		select {
		case request := <-requests:
			return request
		case <-time.After(testTimeout):
			t.Fatal("LLM did not receive a request")
			return llm.ChatRequest{}
		}
	}
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var request llm.ChatRequest
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		requests <- request
		content := `{"message":"Jumping","actions":[{"name":"jump"}]}`
		if len(request.Messages) > 0 && request.Messages[len(request.Messages)-1].Content.AsText() == "another request" {
			content = "Plain reply"
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"choices": []any{map[string]any{"message": map[string]any{"role": "assistant", "content": content}}},
		})
	}))
	defer api.Close()

	cfg := config.DefaultConfig()
	cfg.Action.OSCAddress = socket.LocalAddr().String()
	cfg.Action.Locations = []config.LocationConfig{{Name: "Home"}}
	cfg.Language = "en"
	cfg.Talk.HistorySize = 2
	cfg.Providers = []config.ProviderConfig{{ID: "test", BaseURL: api.URL, Enabled: true}}
	cfg.DefaultRef = &config.ModelRef{ProviderID: "test", Model: "test-model"}
	b := bus.New()
	mc := module.Ctx{Bus: b, Config: &cfg}
	m, err := New(mc)
	if err != nil {
		t.Fatal(err)
	}
	if m.Name() != "npc-action" {
		t.Fatalf("module name = %q", m.Name())
	}

	sub := b.Subscribe()
	defer sub.Close()
	ctx, cancel := context.WithCancel(t.Context())
	done := make(chan error, 1)
	go func() { done <- m.Run(ctx, mc) }()
	defer func() {
		cancel()
		select {
		case err := <-done:
			if err != nil {
				t.Error(err)
			}
		case <-time.After(testTimeout):
			t.Error("module did not shut down")
		}
	}()

	command := func(text string) {
		b.Publish(bus.TopicAction, msgCommand, map[string]string{"text": text})
	}
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	ready := time.NewTimer(testTimeout)
	defer ready.Stop()
waitForReady:
	for {
		select {
		case <-ticker.C:
			command("locations")
		case msg := <-sub.C:
			if msg.Topic == bus.TopicUI && msg.Type == msgActionLog && strings.Contains(msg.Str("text"), "Home") {
				break waitForReady
			}
		case <-ready.C:
			t.Fatal("module did not start")
		}
	}
	ticker.Stop()

	// A new hold cannot start until the cancelled hold has sent its zero.
	command(" FW 60 ")
	expectOSC(t, socket, "/input/Vertical", float32(1))
	command("TL 0")
	expectOSC(t, socket, "/input/Vertical", float32(0))
	expectOSC(t, socket, "/input/LookHorizontal", float32(-1))
	expectOSC(t, socket, "/input/LookHorizontal", float32(0))

	command("reset")
	msg := awaitUI(t, sub, msgPosition, func(bus.Message) bool { return true })
	var pos positionData
	if err := msg.Decode(&pos); err != nil || pos != (positionData{}) {
		t.Fatalf("reset position = %+v, error = %v", pos, err)
	}
	awaitActionLog(t, sub, "Position reset to origin")

	// Reload map data using the bus. Movement and planning keep their
	// startup connections even though the new config supplies neither.
	b.Publish(bus.TopicConfig, bus.MsgConfigUpdated, json.RawMessage(
		`{"action":{"locations":[{"name":"Cafe"}],"routes":[{"name":"Patrol","waypoints":[{"location":"CAFE"}]}]}}`,
	))
	command("route PATROL")
	awaitActionLog(t, sub, "Waypoint 1/1: 'Cafe'")
	awaitActionLog(t, sub, "Route 'Patrol' completed")

	b.Publish(bus.TopicAction, bus.MsgAction, map[string]string{"content": "please jump"})
	awaitActionLog(t, sub, "Jumping")
	expectOSC(t, socket, "/input/Jump", int32(1))
	expectOSC(t, socket, "/input/Jump", int32(0))
	awaitActionLog(t, sub, "[Action] Task sequence completed")
	request := awaitRequest()
	if request.Model != "test-model" || request.ResponseFormat == nil || request.ResponseFormat.Type != "json_object" {
		t.Fatalf("unexpected LLM request: %+v", request)
	}
	if len(request.Messages) != 2 || request.Messages[1].Content.AsText() != "please jump" {
		t.Fatalf("unexpected LLM messages: %+v", request.Messages)
	}
	prompt := request.Messages[0].Content.AsText()
	if !strings.Contains(prompt, "- Cafe (x=0.0, y=0.0, heading=0.0°)") ||
		!strings.Contains(prompt, "- Patrol: CAFE") ||
		!strings.Contains(prompt, "Always write your reply in English.") {
		t.Fatalf("LLM prompt is missing startup language or reloaded map: %s", prompt)
	}

	command("another request")
	awaitActionLog(t, sub, "Plain reply")
	request = awaitRequest()
	if len(request.Messages) != 3 || request.Messages[1].Role != "assistant" ||
		request.Messages[2].Content.AsText() != "another request" {
		t.Fatalf("unexpected trimmed history: %+v", request.Messages)
	}

	command("fw 60")
	expectOSC(t, socket, "/input/Vertical", float32(1))
	cancel()
	expectOSC(t, socket, "/input/Vertical", float32(0))
}
