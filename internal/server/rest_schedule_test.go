package server

import (
	"net/http"
	"os"
	"reflect"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/scheduleprofile"
)

func TestResolveTestAnnouncement(t *testing.T) {
	cfg := config.DefaultConfig()
	saved := config.DefaultAnnouncementConfig()
	saved.Time, saved.Text, saved.ChimeFile, saved.Volume = "09:00", "saved text", "saved.wav", 1
	saved.Actions = []config.ScheduledAction{{Kind: config.ScheduledActionResume}}
	cfg.Scheduler.Announcements = []config.AnnouncementConfig{saved}

	edited := saved
	edited.Text, edited.ChimeFile = "just typed", ""
	edited.Actions = []config.ScheduledAction{{Kind: config.ScheduledActionCommand, Text: "go home"}}
	bgm := saved
	bgm.BGMFile, bgm.BGMVolume, bgm.BGMPlayFull, bgm.BGMEndTime = "bgm/new-track.wav", 0.4, true, "10:30"
	empty := saved
	empty.Actions = []config.ScheduledAction{}
	adHoc := config.DefaultAnnouncementConfig()
	adHoc.Text = "ad hoc"
	actionsOnly := config.DefaultAnnouncementConfig()
	actionsOnly.Actions = []config.ScheduledAction{{Kind: config.ScheduledActionSuspend}}

	for _, tc := range []struct {
		name, body string
		want       config.AnnouncementConfig
	}{
		{"saved index", `{"index":0}`, saved},
		{"unsaved overrides", `{"index":0,"text":"just typed","chime_file":"","actions":[{"kind":"command","text":"go home"}]}`, edited},
		{"unsaved BGM", `{"index":0,"bgm_file":"bgm/new-track.wav","bgm_volume":0.4,"bgm_play_full":true,"bgm_end_time":"10:30"}`, bgm},
		{"empty actions override", `{"index":0,"actions":[]}`, empty},
		{"text only", `{"text":"ad hoc"}`, adHoc},
		{"actions only", `{"actions":[{"kind":"suspend"}]}`, actionsOnly},
	} {
		t.Run(tc.name, func(t *testing.T) {
			req := decodeResponse[schedulerTestRequest](t, []byte(tc.body))
			got, err := resolveTestAnnouncement(req, &cfg)
			if err != nil || !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("announcement %#v, want %#v, error %v", got, tc.want, err)
			}
		})
	}
	for _, body := range []string{`{"index":3}`, `{}`} {
		req := decodeResponse[schedulerTestRequest](t, []byte(body))
		if _, err := resolveTestAnnouncement(req, &cfg); err == nil {
			t.Errorf("expected error for %s", body)
		}
	}
}

func TestScheduleProfileSaveListOverwriteAndDelete(t *testing.T) {
	s := testState(t)
	h := s.routes()
	listPath := "/api/schedule-profiles"
	first := request(t, h, http.MethodPost, listPath, `{"id":"weekday","label":"Weekday"}`, http.StatusOK)
	initial := decodeResponse[object](t, first.Body.Bytes())["profile"].(map[string]any)
	list := request(t, h, http.MethodGet, listPath, "", http.StatusOK)
	items := decodeResponse[[]object](t, list.Body.Bytes())
	if len(items) != 1 || items[0]["id"] != "weekday" || items[0]["label"] != "Weekday" || items[0]["active"] != false {
		t.Fatalf("initial list: %v", items)
	}

	time.Sleep(5 * time.Millisecond)
	save := request(t, h, http.MethodPost, listPath, `{"id":"weekday","label":"Weekday v2"}`, http.StatusOK)
	updated := decodeResponse[object](t, save.Body.Bytes())["profile"].(map[string]any)
	if updated["created_at"] != initial["created_at"] || updated["updated_at"] == initial["updated_at"] ||
		updated["label"] != "Weekday v2" {
		t.Fatalf("updated profile: %v (was %v)", updated, initial)
	}
	list = request(t, h, http.MethodGet, listPath, "", http.StatusOK)
	if items := decodeResponse[[]object](t, list.Body.Bytes()); len(items) != 1 {
		t.Fatalf("overwrite yielded %d profiles", len(items))
	}

	request(t, h, http.MethodDelete, listPath+"/weekday", "", http.StatusOK)
	list = request(t, h, http.MethodGet, listPath, "", http.StatusOK)
	if list.Body.String() != "[]" {
		t.Fatalf("profiles after delete: %s", list.Body)
	}
}

func TestActivateScheduleProfile(t *testing.T) {
	s := testState(t)
	h := s.routes()
	p := scheduleprofile.ScheduleProfile{
		ID: "event", Label: "Event", CreatedAt: "2026-01-01T00:00:00Z", UpdatedAt: "2026-01-01T00:00:00Z",
		Scheduler: config.DefaultSchedulerConfig(),
	}
	p.Scheduler.Enabled = true
	p.Scheduler.Announcements = []config.AnnouncementConfig{{Time: "12:00", Text: "event time"}}
	if err := scheduleprofile.SaveScheduleProfile(s.mc.DataDir, &p); err != nil {
		t.Fatal(err)
	}
	putFile(t, s.mc.ConfigPath, []byte(`{"future_section":{"outside":true},"schedule_profile":{"future_key":true}}`))

	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	request(t, h, http.MethodPost, "/api/schedule-profiles/event/activate", "", http.StatusOK)
	cfg := s.current.Load()
	if !cfg.Scheduler.Enabled || len(cfg.Scheduler.Announcements) != 1 ||
		cfg.Scheduler.Announcements[0].Text != "event time" || cfg.ScheduleProfile.ActiveID != "event" {
		t.Fatalf("active profile: %+v %+v", cfg.Scheduler, cfg.ScheduleProfile)
	}
	msg := receiveBus(t, sub)
	if msg.Topic != bus.TopicConfig || msg.Type != bus.MsgConfigUpdated {
		t.Fatalf("expected config update, got %v", msg)
	}
	data, err := os.ReadFile(s.mc.ConfigPath)
	if err != nil {
		t.Fatal(err)
	}
	doc := decodeResponse[map[string]object](t, data)
	if doc["scheduler"]["enabled"] != true || doc["schedule_profile"]["active_id"] != "event" ||
		doc["schedule_profile"]["future_key"] != true || doc["future_section"]["outside"] != true {
		t.Fatalf("persisted config: %s", data)
	}
	list := request(t, h, http.MethodGet, "/api/schedule-profiles", "", http.StatusOK)
	items := decodeResponse[[]object](t, list.Body.Bytes())
	if len(items) != 1 || items[0]["active"] != true {
		t.Fatalf("profile list: %v", items)
	}
	request(t, h, http.MethodPost, "/api/schedule-profiles/nope/activate", "", http.StatusNotFound)
}

func TestSchedulerImportExportAndFire(t *testing.T) {
	s := testState(t)
	h := s.routes()
	const schedule = `{"enabled":false,"announcements":[{"time":"12:00","text":"","actions":[{"kind":"resume"}]}]}`
	request(t, h, http.MethodPost, "/api/scheduler/import", schedule, http.StatusOK)
	exported := request(t, h, http.MethodGet, "/api/scheduler/export", "", http.StatusOK)
	if exported.Header().Get("Content-Type") != "application/json" || !reflect.DeepEqual(
		decodeResponse[config.SchedulerConfig](t, exported.Body.Bytes()), s.current.Load().Scheduler) {
		t.Fatalf("scheduler export: %s", exported.Body)
	}
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	response := request(t, h, http.MethodPost, "/api/scheduler/test", `{"index":0}`, http.StatusOK)
	got := decodeResponse[object](t, response.Body.Bytes())
	if got["fired"] != true || got["spoke"] != false || got["actions"] != float64(1) {
		t.Fatalf("test outcome: %v", got)
	}
	if msg := receiveBus(t, sub); msg.Topic != bus.TopicInterrupt || msg.Type != bus.MsgResume {
		t.Fatalf("unexpected scheduled message %v", msg)
	}
}
