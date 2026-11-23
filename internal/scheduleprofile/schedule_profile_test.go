package scheduleprofile

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/tik-choco/tc-npc/internal/config"
)

func sampleProfile(id string) ScheduleProfile {
	cfg := config.DefaultSchedulerConfig()
	cfg.Enabled = true
	ann := config.DefaultAnnouncementConfig()
	ann.Time = "09:00"
	ann.Text = "おはようございます"
	cfg.Announcements = []config.AnnouncementConfig{ann}
	return ScheduleProfile{
		ID:        id,
		Label:     "Weekday",
		CreatedAt: "2026-01-01T00:00:00Z",
		UpdatedAt: "2026-01-01T00:00:00Z",
		Scheduler: cfg,
	}
}

func saveSample(t *testing.T, dir, id string) ScheduleProfile {
	t.Helper()
	profile := sampleProfile(id)
	if err := SaveScheduleProfile(dir, &profile); err != nil {
		t.Fatal(err)
	}
	return profile
}

func TestSaveAndLoadRoundTrip(t *testing.T) {
	dir := t.TempDir()
	want := saveSample(t, dir, "weekday")
	loaded, err := LoadScheduleProfile(dir, "weekday")
	if err != nil || !reflect.DeepEqual(loaded, want) {
		t.Fatalf("loaded %+v, err %v, want %+v", loaded, err, want)
	}
}

func TestListReturnsSavedProfilesSortedByID(t *testing.T) {
	dir := t.TempDir()
	saveSample(t, dir, "weekday")
	saveSample(t, dir, "event")
	saveSample(t, dir, "holiday")

	listed, err := ListScheduleProfiles(dir)
	if err != nil {
		t.Fatal(err)
	}
	ids := make([]string, len(listed))
	for i, profile := range listed {
		ids[i] = profile.ID
	}
	if want := []string{"event", "holiday", "weekday"}; !reflect.DeepEqual(ids, want) {
		t.Fatalf("ids = %v, want %v", ids, want)
	}
}

func TestListOnMissingDirectoryReturnsEmpty(t *testing.T) {
	listed, err := ListScheduleProfiles(t.TempDir())
	if err != nil || listed == nil || len(listed) != 0 {
		t.Fatalf("list = %#v, err %v, want an empty list", listed, err)
	}
}

func TestIDsThatWouldEscapeTheFolderAreRejected(t *testing.T) {
	dir := t.TempDir()
	for _, bad := range []string{"../evil", "a/b", "a\\b", "C:evil", "", ".hidden", "a..b"} {
		profile := sampleProfile(bad)
		if err := validateScheduleProfileID(bad); err == nil {
			t.Errorf("validation accepted %q", bad)
		}
		if err := SaveScheduleProfile(dir, &profile); err == nil {
			t.Errorf("save accepted %q", bad)
		}
		if _, err := LoadScheduleProfile(dir, bad); err == nil {
			t.Errorf("load accepted %q", bad)
		}
		if err := DeleteScheduleProfile(dir, bad); err == nil {
			t.Errorf("delete accepted %q", bad)
		}
	}
}

func TestDeleteOfNonexistentIDIsNotAnError(t *testing.T) {
	if err := DeleteScheduleProfile(t.TempDir(), "nope"); err != nil {
		t.Fatal(err)
	}
}

func TestDeleteRemovesSavedProfile(t *testing.T) {
	dir := t.TempDir()
	saveSample(t, dir, "weekday")
	if _, err := LoadScheduleProfile(dir, "weekday"); err != nil {
		t.Fatal(err)
	}
	if err := DeleteScheduleProfile(dir, "weekday"); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadScheduleProfile(dir, "weekday"); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("load after delete: %v", err)
	}
}

func TestListSkipsCorruptFile(t *testing.T) {
	dir := t.TempDir()
	saveSample(t, dir, "good")
	if err := os.WriteFile(scheduleProfilePath(dir, "bad"), []byte("{ not valid json"), 0o666); err != nil {
		t.Fatal(err)
	}
	listed, err := ListScheduleProfiles(dir)
	if err != nil || len(listed) != 1 || listed[0].ID != "good" {
		t.Fatalf("list = %+v, err %v, want only good", listed, err)
	}
}

func TestScheduleProfileJSONDefaultsAndValidation(t *testing.T) {
	want := ScheduleProfile{ID: "weekday", Scheduler: config.DefaultSchedulerConfig()}
	for _, valid := range []string{`{"id":"weekday","ignored":null}`, `["weekday"]`} {
		var got ScheduleProfile
		if err := json.Unmarshal([]byte(valid), &got); err != nil || !reflect.DeepEqual(got, want) {
			t.Fatalf("unmarshal %s: got %+v, err %v, want %+v", valid, got, err, want)
		}
	}
	for _, invalid := range []string{
		`null`, `{}`, `{"ID":"weekday"}`, `{"id":null}`, `{"id":"a","id":"b"}`,
		`{"id":"weekday","label":null}`, `{"id":"weekday","scheduler":null}`,
	} {
		var got ScheduleProfile
		if err := json.Unmarshal([]byte(invalid), &got); err == nil {
			t.Errorf("unmarshal accepted %s", invalid)
		}
	}
}

func TestSaveReplacesProfileUsingRustFileShape(t *testing.T) {
	dir := t.TempDir()
	profile := saveSample(t, dir, "weekday")
	profile.Label = "Weekend"
	profile.Scheduler = config.DefaultSchedulerConfig()
	if err := SaveScheduleProfile(dir, &profile); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "schedule_profiles", "weekday.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	const want = `{
  "id": "weekday",
  "label": "Weekend",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z",
  "scheduler": {
    "enabled": false,
    "announcements": [],
    "defaults": {
      "chime_file": "",
      "volume": 1,
      "bgm_volume": 0.5
    }
  }
}`
	if string(data) != want {
		t.Fatalf("saved JSON = %s, want %s", data, want)
	}
}
