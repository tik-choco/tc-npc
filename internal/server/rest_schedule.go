package server

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/scheduleprofile"
	"github.com/tik-choco/tc-npc/internal/scheduler"
)

type schedulerTestRequest struct {
	Index       *uint                     `json:"index"`
	Text        *string                   `json:"text"`
	ChimeFile   *string                   `json:"chime_file"`
	Actions     *[]config.ScheduledAction `json:"actions"`
	BGMFile     *string                   `json:"bgm_file"`
	BGMVolume   *float32                  `json:"bgm_volume"`
	BGMPlayFull *bool                     `json:"bgm_play_full"`
	BGMEndTime  *string                   `json:"bgm_end_time"`
}

func (req *schedulerTestRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func valueOr[T any](value *T, fallback T) T {
	if value != nil {
		return *value
	}
	return fallback
}

func resolveTestAnnouncement(req schedulerTestRequest, cfg *config.Config) (config.AnnouncementConfig, error) {
	ann := config.DefaultAnnouncementConfig()
	switch {
	case req.Index != nil:
		index := *req.Index
		if index >= uint(len(cfg.Scheduler.Announcements)) {
			return ann, fmt.Errorf("announcement index %d out of range", index)
		}
		ann = cfg.Scheduler.Announcements[index]
	case req.Text == nil && req.Actions == nil:
		return ann, errors.New("either `index`, `text` or `actions` is required")
	}

	ann.Text = valueOr(req.Text, ann.Text)
	ann.ChimeFile = valueOr(req.ChimeFile, ann.ChimeFile)
	ann.Actions = valueOr(req.Actions, ann.Actions)
	ann.BGMFile = valueOr(req.BGMFile, ann.BGMFile)
	ann.BGMVolume = valueOr(req.BGMVolume, ann.BGMVolume)
	ann.BGMPlayFull = valueOr(req.BGMPlayFull, ann.BGMPlayFull)
	ann.BGMEndTime = valueOr(req.BGMEndTime, ann.BGMEndTime)
	return ann, nil
}

func (s *appState) apiSchedulerTest(w http.ResponseWriter, r *http.Request) {
	var req schedulerTestRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return
	}
	cfg := s.current.Load()
	ann, err := resolveTestAnnouncement(req, cfg)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	outcome := scheduler.FireAnnouncement(s.mc.Bus, &ann, &cfg.Scheduler.Defaults)
	writeJSON(w, object{"ok": true, "fired": outcome.Fired(), "spoke": outcome.Spoke, "actions": outcome.Actions})
}

func (s *appState) apiSchedulerExport(w http.ResponseWriter, _ *http.Request) {
	var data bytes.Buffer
	enc := json.NewEncoder(&data)
	enc.SetIndent("", "  ")
	enc.SetEscapeHTML(false)
	if err := enc.Encode(s.current.Load().Scheduler); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeBytes(w, "application/json", bytes.TrimSuffix(data.Bytes(), []byte{10}))
}

func (s *appState) apiSchedulerImport(w http.ResponseWriter, r *http.Request) {
	var incoming config.SchedulerConfig
	if !readJSONRequest(w, r, "invalid scheduler config: ", &incoming) {
		return
	}
	s.configMu.Lock()
	defer s.configMu.Unlock()
	updated := *s.current.Load()
	updated.Scheduler = incoming
	if err := updated.Save(s.mc.ConfigPath); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.publishConfig(&updated)
	writeJSON(w, object{"ok": true, "scheduler": incoming})
}

func scheduleProfileSummary(p scheduleprofile.ScheduleProfile, activeID string) object {
	return object{
		"id": p.ID, "label": p.Label, "active": p.ID == activeID,
		"created_at": p.CreatedAt, "updated_at": p.UpdatedAt,
	}
}

func (s *appState) apiListScheduleProfiles(w http.ResponseWriter, _ *http.Request) {
	profiles, err := scheduleprofile.ListScheduleProfiles(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	active := s.current.Load().ScheduleProfile.ActiveID
	out := make([]object, 0, len(profiles))
	for _, p := range profiles {
		out = append(out, scheduleProfileSummary(p, active))
	}
	writeJSON(w, out)
}

type saveScheduleProfileRequest struct {
	ID    string `json:"id" required:"true"`
	Label string `json:"label" required:"true"`
}

func (req *saveScheduleProfileRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func (s *appState) apiSaveScheduleProfile(w http.ResponseWriter, r *http.Request) {
	var req saveScheduleProfileRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return
	}
	if strings.TrimSpace(req.ID) == "" || strings.TrimSpace(req.Label) == "" {
		writeError(w, http.StatusBadRequest, "id and label are required")
		return
	}

	s.configMu.Lock()
	defer s.configMu.Unlock()
	cfg := s.current.Load()
	now := utcTimestamp()
	p := scheduleprofile.ScheduleProfile{
		ID: req.ID, Label: req.Label, CreatedAt: now, UpdatedAt: now, Scheduler: cfg.Scheduler,
	}
	if old, err := scheduleprofile.LoadScheduleProfile(s.mc.DataDir, req.ID); err == nil {
		p.CreatedAt = old.CreatedAt
	}
	if err := scheduleprofile.SaveScheduleProfile(s.mc.DataDir, &p); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, object{"ok": true, "profile": scheduleProfileSummary(p, cfg.ScheduleProfile.ActiveID)})
}

func (s *appState) apiActivateScheduleProfile(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	s.configMu.Lock()
	defer s.configMu.Unlock()
	p, err := scheduleprofile.LoadScheduleProfile(s.mc.DataDir, id)
	if err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	err = s.editConfigFile(func(doc object) {
		doc["scheduler"] = p.Scheduler
		setConfigField(doc, "schedule_profile", "active_id", id)
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	updated := *s.current.Load()
	updated.Scheduler = p.Scheduler
	updated.ScheduleProfile.ActiveID = id
	s.publishConfig(&updated)
	writeJSON(w, object{"ok": true, "scheduler": updated.Scheduler})
}

func (s *appState) apiDeleteScheduleProfile(w http.ResponseWriter, r *http.Request) {
	s.configMu.Lock()
	defer s.configMu.Unlock()
	if err := scheduleprofile.DeleteScheduleProfile(s.mc.DataDir, r.PathValue("id")); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, object{"ok": true})
}

func utcTimestamp() string {
	now := time.Now().UTC()
	layout := "2006-01-02T15:04:05-07:00"
	switch {
	case now.Nanosecond() == 0:
	case now.Nanosecond()%1_000_000 == 0:
		layout = "2006-01-02T15:04:05.000-07:00"
	case now.Nanosecond()%1_000 == 0:
		layout = "2006-01-02T15:04:05.000000-07:00"
	default:
		layout = "2006-01-02T15:04:05.000000000-07:00"
	}
	return now.Format(layout)
}
