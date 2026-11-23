package tui

import (
	"strconv"
	"strings"
	"testing"
	"time"
)

func TestDefaultApp(t *testing.T) {
	a := app{}
	if a.connection != connecting || a.inConfig() || a.focus != focusInput || a.pending || len(a.chat) != 0 {
		t.Fatalf("unexpected initial app: %+v", a)
	}
}

func TestHelloUpdatesVersionModulesAndCharacter(t *testing.T) {
	a := app{}
	a.applyServerMsg(mustParseFrame(t, `{"type":"hello","version":"1","modules":`+modulesJSON+`,"character":{"id":"c1","name":"Rin"}}`))
	if a.version != "1" || !a.modules.Talk || a.character == nil || a.character.Name != "Rin" {
		t.Fatalf("hello did not update app: %+v", a)
	}
}

func TestUserEchoKeepsPendingAndAssistantClearsIt(t *testing.T) {
	a := app{pending: true}
	a.applyServerMsg(mustParseFrame(t, `{"type":"chat","role":"user","text":"hi","ts":0}`))
	if !a.pending {
		t.Fatal("user echo cleared pending")
	}
	a.applyServerMsg(mustParseFrame(t, `{"type":"chat","role":"assistant","text":"yo","ts":0}`))
	if a.pending || len(a.chat) != 2 {
		t.Fatalf("assistant echo left pending=%v, chat=%+v", a.pending, a.chat)
	}
}

func TestSilentClearsPendingAndRecordsReason(t *testing.T) {
	a := app{pending: true}
	a.applyServerMsg(mustParseFrame(t, `{"type":"silent","reason":"declined","ts":0}`))
	if a.pending || len(a.chat) != 1 || a.chat[0] != (chatRow{text: "declined", silent: true}) {
		t.Fatalf("silent left pending=%v, chat=%+v", a.pending, a.chat)
	}
}

func TestInputAcceptedSetsPending(t *testing.T) {
	a := app{}
	a.applyServerMsg(mustParseFrame(t, `{"type":"inputAccepted","requestId":"r1"}`))
	if !a.pending {
		t.Fatal("ack did not set pending")
	}
}

func TestServerErrorsSetStatusLineAndClearPending(t *testing.T) {
	for _, tc := range []struct {
		raw  string
		want string
	}{
		{`{"type":"error","message":"boom"}`, "error: boom"},
		{`{"type":"response","requestId":"r1","status":"error","message":"nope"}`, "error: nope"},
		{`{"type":"response","requestId":"r1","status":"error"}`, "error: unknown error"},
	} {
		a := app{pending: true}
		a.applyServerMsg(mustParseFrame(t, tc.raw))
		if a.pending || a.statusLine != tc.want {
			t.Errorf("%s left pending=%v, status=%q, want %q", tc.raw, a.pending, a.statusLine, tc.want)
		}
	}
}

func TestLogFrameRouting(t *testing.T) {
	a := app{}
	for _, tc := range []struct {
		raw  string
		want logRow
	}{
		{`{"type":"translation","text":"hi"}`, logRow{"unhandled", "translation"}},
		{`{"type":"sense","kind":"speech","text":"聞いた","ts":1}`, logRow{"sense:speech", "聞いた"}},
		{`{"type":"memory","kind":"short","text":"覚えた"}`, logRow{"memory:short", "覚えた"}},
		{`{"type":"actionLog","text":"moved"}`, logRow{"action", "moved"}},
		{`{"type":"response","requestId":"r1","status":"done","text":"reply"}`, logRow{"response", "reply"}},
		{`{"type":"error","message":"boom"}`, logRow{"error", "boom"}},
	} {
		a.applyServerMsg(mustParseFrame(t, tc.raw))
		if n := len(a.log); n == 0 || a.log[n-1] != tc.want || len(a.chat) != 0 {
			t.Fatalf("%s gave log=%+v, chat=%+v, want last log %+v", tc.raw, a.log, a.chat, tc.want)
		}
	}
}

func TestChatRowsCappedAtMaxRows(t *testing.T) {
	a := app{}
	for i := range maxRows + 10 {
		text := strconv.Itoa(i)
		a.applyServerMsg(serverMsg{kind: "chat", role: "user", text: &text})
	}
	if len(a.chat) != maxRows || a.chat[0].text != "10" {
		t.Fatalf("unexpected scrollback of size %d starting with %+v", len(a.chat), a.chat[:1])
	}
}

func TestMainFocusWraps(t *testing.T) {
	a := app{}
	for _, tc := range []struct {
		step int
		want focus
	}{{1, focusChat}, {1, focusLog}, {1, focusInput}, {-1, focusLog}} {
		a.cycleFocus(tc.step)
		if a.focus != tc.want {
			t.Fatalf("step %d gave focus %d, want %d", tc.step, a.focus, tc.want)
		}
	}
}

func TestContinuousTelemetryDoesNotReachLog(t *testing.T) {
	a := app{}
	for range 50 {
		for _, kind := range []string{"volume", "speakingLevel", "speaking", "position", "affect"} {
			a.applyServerMsg(serverMsg{kind: kind, unknown: true})
		}
	}
	if len(a.log) != 0 {
		t.Fatalf("telemetry reached log: %+v", a.log)
	}
	a.applyServerMsg(serverMsg{kind: "vrmPose", unknown: true})
	if len(a.log) != 1 || a.log[0].text != "vrmPose" {
		t.Fatalf("one-off frame missing from log: %+v", a.log)
	}
}

func TestConfigFocusWraps(t *testing.T) {
	a := app{}
	a.toggleView()
	for _, tc := range []struct {
		step int
		want focus
	}{{0, focusModules}, {1, focusCharacters}, {1, focusModules}, {-1, focusCharacters}} {
		a.cycleFocus(tc.step)
		if !a.inConfig() || a.focus != tc.want {
			t.Fatalf("step %d gave focus %d, want %d", tc.step, a.focus, tc.want)
		}
	}
}

func TestToggleViewRestoresFirstFocus(t *testing.T) {
	a := app{focus: focusLog}
	a.toggleView()
	if !a.inConfig() || a.focus != focusModules {
		t.Fatal("Config did not focus Modules")
	}
	a.cycleFocus(1)
	a.toggleView()
	if a.inConfig() || a.focus != focusInput {
		t.Fatal("Main did not restore Input focus")
	}
}

func TestReturnToMain(t *testing.T) {
	a := app{}
	a.returnToMain()
	if a.inConfig() || a.notice != "" || len(a.eventLog) != 0 {
		t.Fatal("returnToMain changed Main state")
	}
	a.toggleView()
	if !a.inConfig() {
		t.Fatal("did not enter Config")
	}
	a.returnToMain()
	if a.inConfig() || a.focus != focusInput {
		t.Fatal("returnToMain did not restore Main")
	}
}

func TestEventLogCapAndBlankMessage(t *testing.T) {
	a := app{}
	a.addEventLog("   ")
	if len(a.eventLog) != 0 {
		t.Fatal("recorded a blank event")
	}
	for i := range eventLogLimit + 5 {
		a.addEventLog(strconv.Itoa(i))
	}
	if len(a.eventLog) != eventLogLimit || a.eventLog[0] != "5" {
		t.Fatalf("event cap produced %d events, first %q", len(a.eventLog), a.eventLog[0])
	}
}

func TestSetConnectionUpdatesAndLogsOnlyTransitions(t *testing.T) {
	a := app{}
	a.setConnection(connected)
	if a.connection != connected || len(a.eventLog) != 1 || !strings.Contains(a.eventLog[0], connected.label()) {
		t.Fatalf("unexpected transition: %v, %v", a.connection, a.eventLog)
	}
	a.setConnection(connected)
	if len(a.eventLog) != 1 {
		t.Fatalf("logged redundant state: %v", a.eventLog)
	}
}

func TestErrorExpiresAfterTTL(t *testing.T) {
	a := app{}
	now := time.Now()
	a.setError("boom", now)
	if a.operatorError == nil {
		t.Fatal("did not set error")
	}
	a.tick(now.Add(5 * time.Second))
	if a.operatorError == nil {
		t.Fatal("expired too soon")
	}
	a.tick(now.Add(11 * time.Second))
	if a.operatorError != nil {
		t.Fatal("did not expire")
	}
}

func TestNoticePersistsUntilCleared(t *testing.T) {
	a := app{}
	a.setNotice("しきい値を更新")
	a.tick(time.Now().Add(time.Hour))
	if a.notice != "しきい値を更新" {
		t.Fatalf("unexpected notice after tick: %q", a.notice)
	}
	a.clearNotice()
	if a.notice != "" {
		t.Fatalf("notice survived clearing: %q", a.notice)
	}
}

func TestStatusUpdatesOrderedModuleStates(t *testing.T) {
	a := app{}
	a.applyServerMsg(mustParseFrame(t, `{"type":"status","modules":{"talk":true,"memory":false,"speech":true,"vision":false,"action":false,"scheduler":false,"translation":false}}`))
	want := [7]moduleState{{"talk", true}, {"memory", false}, {"speech", true}, {"vision", false}, {"action", false}, {"scheduler", false}, {"translation", false}}
	if got := a.moduleStates(); got != want {
		t.Fatalf("module states = %v, want %v", got, want)
	}
}

func TestSystemActivity(t *testing.T) {
	for _, tc := range []struct {
		state   connectionState
		pending bool
		want    systemActivity
	}{
		{disconnected, false, activityOffline},
		{reconnecting, false, activityConnecting},
		{connecting, false, activityConnecting},
		{connected, false, activityIdle},
		{connected, true, activityWaiting},
	} {
		a := app{connection: tc.state, pending: tc.pending}
		if got := a.systemActivity(); got != tc.want {
			t.Errorf("state %v pending %v: got activity %v, want %v", tc.state, tc.pending, got, tc.want)
		}
	}
}

func TestSendInputTrimsAndClears(t *testing.T) {
	for input, want := range map[string]string{"  こんにちは  ": "こんにちは", "   ": ""} {
		a := app{input: input}
		got := a.takeInputForSend()
		if got != want || a.input != "" {
			t.Errorf("input %q sent %q, left %q, want %q and cleared", input, got, a.input, want)
		}
	}
}
