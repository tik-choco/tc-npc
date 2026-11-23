package config

import "encoding/json"

// AnnouncementConfig is a daily announcement and its accompanying actions.
type AnnouncementConfig struct {
	// Daily time in HH:MM or HH:MM:SS.
	Time      string `json:"time" required:"true"`
	Text      string `json:"text" required:"true"`
	ChimeFile string `json:"chime_file"`
	// A nonpositive setting inherits the scheduler-wide volume.
	Volume  float32 `json:"volume"`
	BGMFile string  `json:"bgm_file"`
	// A nonpositive setting inherits the scheduler-wide BGM volume.
	BGMVolume float32 `json:"bgm_volume"`
	// Continues BGM until its natural end or the configured cutoff.
	BGMPlayFull bool `json:"bgm_play_full"`
	// Optional BGM playback cutoff in M:SS.
	BGMEndTime string `json:"bgm_end_time"`
	// Extra bus messages published in order, even when Text is empty.
	Actions []ScheduledAction `json:"actions"`
}

// DefaultAnnouncementConfig returns the Rust defaults for AnnouncementConfig.
func DefaultAnnouncementConfig() AnnouncementConfig {
	return AnnouncementConfig{
		Actions: []ScheduledAction{},
	}
}

func (c *AnnouncementConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultAnnouncementConfig())
}

func (c AnnouncementConfig) MarshalJSON() ([]byte, error) {
	type wire AnnouncementConfig
	c.Actions = nonNilSlice(c.Actions)
	return json.Marshal(wire(c))
}

// SchedulerDefaults provides fallbacks for the schedule's chime and volumes.
type SchedulerDefaults struct {
	ChimeFile string  `json:"chime_file"`
	Volume    float32 `json:"volume"`
	BGMVolume float32 `json:"bgm_volume"`
}

// DefaultSchedulerDefaults returns the Rust defaults for SchedulerDefaults.
func DefaultSchedulerDefaults() SchedulerDefaults {
	return SchedulerDefaults{
		Volume:    1.0,
		BGMVolume: 0.5,
	}
}

func (c *SchedulerDefaults) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultSchedulerDefaults())
}

// SchedulerConfig holds the active schedule.
type SchedulerConfig struct {
	Enabled       bool                 `json:"enabled"`
	Announcements []AnnouncementConfig `json:"announcements"`
	Defaults      SchedulerDefaults    `json:"defaults"`
}

// DefaultSchedulerConfig returns the Rust defaults for SchedulerConfig.
func DefaultSchedulerConfig() SchedulerConfig {
	return SchedulerConfig{
		Announcements: []AnnouncementConfig{},
		Defaults:      DefaultSchedulerDefaults(),
	}
}

func (c *SchedulerConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultSchedulerConfig())
}

func (c SchedulerConfig) MarshalJSON() ([]byte, error) {
	type wire SchedulerConfig
	c.Announcements = nonNilSlice(c.Announcements)
	return json.Marshal(wire(c))
}

// ScheduleProfileState identifies the profile whose schedule was copied into Scheduler.
type ScheduleProfileState struct {
	ActiveID string `json:"active_id"`
}

func (c *ScheduleProfileState) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, ScheduleProfileState{})
}
