//! Scheduler module (ports Go `agent-scheduler`, see
//! `internal/notifier/scheduler.go` in the original service).
//!
//! Reads `config.scheduler.announcements` (`{time, text, chime_file,
//! volume, actions, bgm_file, bgm_volume, bgm_play_full, bgm_end_time}`,
//! `time` = `"HH:MM"` or `"HH:MM:SS"`, daily recurrence — the Go original
//! turned these into 6-field cron expressions via `robfig/cron`; here we
//! hand-roll the same "next local-time occurrence" semantics with `chrono`
//! instead of pulling in a cron crate). Unlike the Go service this module
//! does **not** play audio itself: firing an announcement just publishes a
//! `tts` message on `agent:interrupt` and `npc-speech` plays the chime,
//! speaks the text, and (if `bgm_file` is set) plays background music
//! alongside it. `chime_file`/`volume`/`bgm_volume` fall back to
//! `config.scheduler.defaults` when an announcement leaves them unset (`<=
//! 0.0` for the volumes, empty string for the chime) — see
//! [`npc_core::config::AnnouncementConfig`]'s `effective_*` methods, which
//! `fire_announcement` resolves before publishing so `npc-speech` never has
//! to know about the defaults itself.
//!
//! Beyond speaking, an announcement can carry `actions` — the port of Go
//! `agent-scheduler`'s `redis_actions`, which published arbitrary
//! `{channel, type, payload}` messages so a scheduled entry could move the
//! avatar or resume a suspended agent instead of (or as well as) talking.
//! See [`npc_core::config::ScheduledAction`] for the variants and
//! [`execute_action`] for the envelope each one publishes.
//!
//! Config hot-reload: this module is always spawned (regardless of
//! `scheduler.enabled` — see `src/main.rs`), and the schedule is recomputed
//! whenever a [`npc_core::bus::msg::CONFIG_UPDATED`] message arrives on
//! [`npc_core::bus::topic::CONFIG`] (published after `PUT /api/config`
//! persists a new config). If `scheduler.enabled` is `false`, or there are
//! no valid `scheduler.announcements`, the module simply has nothing to
//! wait on but shutdown/config updates until one arrives.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeZone};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use npc_core::config::{AnnouncementConfig, ScheduledAction, SchedulerDefaults};
use npc_core::{msg, topic, Config, Module, ModuleCtx};

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(SchedulerModule))
}

struct SchedulerModule;

#[async_trait]
impl Module for SchedulerModule {
    fn name(&self) -> &'static str {
        "npc-scheduler"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let mut entries = compute_schedule(&ctx.config);
        let mut defaults = ctx.config.scheduler.defaults.clone();
        log_schedule(&entries);

        let mut rx = ctx.bus.subscribe();

        loop {
            // With no entries there's nothing to sleep towards — wait
            // indefinitely for shutdown or a config update that might add
            // some. `tokio::time::sleep` with a `far_future` duration keeps
            // the `select!` shape uniform without a separate branch.
            let wake_at = entries.iter().map(|e| e.next).min();
            let duration = match wake_at {
                Some(wake_at) => (wake_at - Local::now()).to_std().unwrap_or(Duration::ZERO),
                None => far_future(),
            };

            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-scheduler: shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(duration) => {
                    let Some(wake_at) = wake_at else { continue };
                    // Fire every entry due at (or before, to absorb
                    // scheduling jitter) `wake_at` — this is what makes
                    // multiple announcements configured for the same time
                    // fire together.
                    for entry in entries.iter_mut() {
                        if entry.next <= wake_at {
                            fire_announcement(&ctx.bus, &entry.config, &defaults);
                            entry.next += chrono::Duration::hours(24);
                        }
                    }
                }
                received = rx.recv() => {
                    match received {
                        Ok(bus_msg) => {
                            if bus_msg.topic == topic::CONFIG && bus_msg.env.r#type == msg::CONFIG_UPDATED {
                                match serde_json::from_value::<Config>(bus_msg.env.payload) {
                                    Ok(new_config) => {
                                        entries = compute_schedule(&new_config);
                                        defaults = new_config.scheduler.defaults.clone();
                                        tracing::info!("npc-scheduler: config updated, schedule recomputed");
                                        log_schedule(&entries);
                                    }
                                    Err(err) => {
                                        tracing::warn!(error = %err, "npc-scheduler: failed to deserialize updated config, keeping current schedule");
                                    }
                                }
                            }
                        }
                        Err(RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "npc-scheduler: bus receiver lagged, some messages dropped");
                        }
                        Err(RecvError::Closed) => {
                            tracing::info!("npc-scheduler: bus closed, shutting down");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// A duration long enough that `tokio::time::sleep` effectively never fires
/// on its own (a plain "no timer" branch would need a separate `select!`
/// arm per state, which is more code than just picking a very long sleep).
/// ~10 years; still short enough to avoid `Duration` overflow when added to
/// `Instant::now()` internally.
fn far_future() -> Duration {
    Duration::from_secs(10 * 365 * 24 * 60 * 60)
}

/// Ports the announcement -> next-occurrence computation out of `run` so it
/// can be re-run whenever a `CONFIG_UPDATED` message arrives. Returns an
/// empty `Vec` if `scheduler.enabled` is `false` or there are no valid
/// announcements — either way, `run`'s loop just idles on shutdown/bus
/// events until a future config update supplies some.
fn compute_schedule(config: &Config) -> Vec<ScheduledAnnouncement> {
    if !config.scheduler.enabled {
        tracing::info!("npc-scheduler: scheduler disabled, idling until config update");
        return Vec::new();
    }

    let now = Local::now();
    let entries: Vec<ScheduledAnnouncement> = config
        .scheduler
        .announcements
        .iter()
        .filter_map(|ann| match parse_time(&ann.time) {
            Some(time) => Some(ScheduledAnnouncement {
                config: ann.clone(),
                next: next_occurrence(time, now),
            }),
            None => {
                tracing::warn!(
                    time = %ann.time,
                    "npc-scheduler: invalid announcement time (expected HH:MM or HH:MM:SS), skipping"
                );
                None
            }
        })
        .collect();

    if entries.is_empty() {
        tracing::info!("npc-scheduler: no valid announcements configured, idling until config update");
    }

    entries
}

fn log_schedule(entries: &[ScheduledAnnouncement]) {
    for entry in entries {
        tracing::info!(time = %entry.config.time, next = %entry.next, "npc-scheduler: scheduled announcement");
    }
}

struct ScheduledAnnouncement {
    config: AnnouncementConfig,
    next: DateTime<Local>,
}

/// Parse `"HH:MM"` (seconds implicitly `:00`) or `"HH:MM:SS"` into a
/// [`NaiveTime`]. Returns `None` for anything else (wrong field count,
/// non-numeric components, out-of-range values) so callers can warn and
/// skip rather than crash.
fn parse_time(s: &str) -> Option<NaiveTime> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m] => (h.parse().ok()?, m.parse().ok()?, 0),
        [h, m, s] => (h.parse().ok()?, m.parse().ok()?, s.parse().ok()?),
        _ => return None,
    };
    NaiveTime::from_hms_opt(h, m, sec)
}

/// The next local-time `DateTime` at or after `from` whose time-of-day is
/// `time` — today if `time` hasn't happened yet, tomorrow otherwise (or if
/// it exactly equals `from`, so a restart lands exactly on an announcement
/// time doesn't immediately re-fire it).
fn next_occurrence(time: NaiveTime, from: DateTime<Local>) -> DateTime<Local> {
    let candidate = local_datetime(NaiveDateTime::new(from.date_naive(), time));
    if candidate > from {
        candidate
    } else {
        candidate + chrono::Duration::days(1)
    }
}

/// Resolve a naive local datetime to `DateTime<Local>`, handling DST
/// ambiguity/gaps without panicking (falls back to the earlier of two
/// ambiguous offsets, or a UTC-as-local approximation for the vanishingly
/// rare nonexistent "spring forward" instant).
fn local_datetime(naive: NaiveDateTime) -> DateTime<Local> {
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => Local.from_utc_datetime(&naive),
    }
}

/// What [`fire_announcement`] actually did, so callers (the web UI's test
/// button in particular) can tell "spoke a line", "ran 2 actions" and "this
/// entry is a no-op" apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FireOutcome {
    /// A `tts` interrupt was published for the announcement's own `text`.
    pub spoke: bool,
    /// How many `actions` entries were published.
    pub actions: usize,
}

impl FireOutcome {
    /// Whether anything at all was published.
    pub fn fired(&self) -> bool {
        self.spoke || self.actions > 0
    }
}

/// Publish `ann`'s TTS line and its `actions` — the one place an announcement
/// turns into bus messages, whether it was reached by the clock or triggered
/// manually. `npc-server`'s `POST /api/scheduler/test` calls this so the web
/// UI's test button exercises the real firing path rather than a lookalike
/// (the Go original does the same: its TUI's `[t] Test Playback` routes
/// through `TestTriggerIndex` -> `TriggerAnnouncement`).
///
/// Text and actions are independent: a text-only entry just speaks, an
/// actions-only entry (empty `text`) just acts, and an entry with both does
/// both — matching the Go original, whose `redis_actions` fired regardless of
/// whether `text` was set.
pub fn fire_announcement(
    bus: &npc_core::bus::Bus,
    ann: &AnnouncementConfig,
    defaults: &SchedulerDefaults,
) -> FireOutcome {
    let mut outcome = FireOutcome::default();

    if ann.text.is_empty() {
        tracing::debug!(time = %ann.time, "npc-scheduler: announcement has no text, skipping tts");
    } else {
        tracing::info!(time = %ann.time, text = %ann.text, "npc-scheduler: firing announcement");
        bus.publish(
            topic::INTERRUPT,
            msg::TTS,
            json!({
                "content": ann.text,
                "chime_file": ann.effective_chime_file(defaults),
                "volume": ann.effective_volume(defaults),
                "bgm_file": ann.bgm_file,
                "bgm_volume": ann.effective_bgm_volume(defaults),
                "bgm_play_full": ann.bgm_play_full,
                "bgm_end_time": ann.bgm_end_time,
            }),
        );
        outcome.spoke = true;
    }

    for action in &ann.actions {
        if execute_action(bus, action) {
            outcome.actions += 1;
        }
    }

    if !outcome.fired() {
        tracing::debug!(time = %ann.time, "npc-scheduler: no-op announcement (no text, no actions)");
    }

    outcome
}

/// Publish the one bus envelope `action` stands for. Returns whether anything
/// was published — an action whose only field is blank (an unfinished row in
/// the web UI) is skipped rather than published as an empty command.
pub fn execute_action(bus: &npc_core::bus::Bus, action: &ScheduledAction) -> bool {
    match action {
        ScheduledAction::Speak { content, chime_file } => {
            if content.is_empty() && chime_file.is_empty() {
                return false;
            }
            bus.publish(
                topic::INTERRUPT,
                msg::TTS,
                json!({ "content": content, "chime_file": chime_file }),
            );
        }
        ScheduledAction::Action { content } => {
            if content.trim().is_empty() {
                return false;
            }
            bus.publish(topic::ACTION, msg::ACTION, json!({ "content": content }));
        }
        ScheduledAction::Command { text } => {
            if text.trim().is_empty() {
                return false;
            }
            // `"command"` is npc-action's local message type (see its
            // `MSG_TYPE_COMMAND`), not one of `npc_core::bus::msg`'s shared
            // constants, so it's spelled out here too.
            bus.publish(topic::ACTION, "command", json!({ "text": text }));
        }
        ScheduledAction::Chat { content } => {
            if content.trim().is_empty() {
                return false;
            }
            bus.publish(topic::SENSE, msg::SPEECH, json!({ "content": content }));
        }
        ScheduledAction::Suspend => bus.publish(topic::INTERRUPT, msg::SUSPEND, json!({})),
        ScheduledAction::Resume => bus.publish(topic::INTERRUPT, msg::RESUME, json!({})),
        ScheduledAction::Raw {
            topic,
            msg_type,
            payload,
        } => {
            if topic.trim().is_empty() || msg_type.trim().is_empty() {
                return false;
            }
            bus.publish(topic, msg_type, payload.clone());
        }
    }

    tracing::info!(action = ?action, "npc-scheduler: firing scheduled action");
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_hh_mm() {
        assert_eq!(parse_time("09:30"), NaiveTime::from_hms_opt(9, 30, 0));
    }

    #[test]
    fn parses_hh_mm_ss() {
        assert_eq!(parse_time("09:30:15"), NaiveTime::from_hms_opt(9, 30, 15));
    }

    #[test]
    fn rejects_invalid_times() {
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("09:70"), None);
        assert_eq!(parse_time("not-a-time"), None);
        assert_eq!(parse_time("09"), None);
        assert_eq!(parse_time("09:30:15:99"), None);
    }

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, s).single().unwrap()
    }

    #[test]
    fn next_occurrence_later_today() {
        let from = local(2026, 7, 24, 10, 0, 0);
        let time = NaiveTime::from_hms_opt(15, 0, 0).unwrap();
        let next = next_occurrence(time, from);
        assert_eq!(next, local(2026, 7, 24, 15, 0, 0));
    }

    #[test]
    fn next_occurrence_already_passed_today_rolls_to_tomorrow() {
        let from = local(2026, 7, 24, 10, 0, 0);
        let time = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let next = next_occurrence(time, from);
        assert_eq!(next, local(2026, 7, 25, 9, 0, 0));
    }

    #[test]
    fn next_occurrence_exact_now_rolls_to_tomorrow() {
        let from = local(2026, 7, 24, 10, 0, 0);
        let time = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let next = next_occurrence(time, from);
        assert_eq!(next, local(2026, 7, 25, 10, 0, 0));
    }

    #[test]
    fn next_occurrence_respects_seconds_precision() {
        let from = local(2026, 7, 24, 9, 0, 30);
        let time = NaiveTime::from_hms_opt(9, 0, 45).unwrap();
        let next = next_occurrence(time, from);
        assert_eq!(next, local(2026, 7, 24, 9, 0, 45));
    }

    fn announcement(time: &str) -> AnnouncementConfig {
        AnnouncementConfig {
            time: time.to_string(),
            text: "hello".to_string(),
            volume: 1.0,
            ..Default::default()
        }
    }

    fn defaults() -> SchedulerDefaults {
        SchedulerDefaults::default()
    }

    #[test]
    fn compute_schedule_disabled_yields_no_entries_even_with_announcements() {
        let mut config = Config::default();
        config.scheduler.enabled = false;
        config.scheduler.announcements = vec![announcement("09:00")];
        assert!(compute_schedule(&config).is_empty());
    }

    #[test]
    fn compute_schedule_enabled_skips_invalid_and_keeps_valid_entries() {
        let mut config = Config::default();
        config.scheduler.enabled = true;
        config.scheduler.announcements = vec![announcement("09:00"), announcement("not-a-time")];
        let entries = compute_schedule(&config);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].config.time, "09:00");
    }

    #[test]
    fn compute_schedule_enabled_with_no_announcements_is_empty() {
        let mut config = Config::default();
        config.scheduler.enabled = true;
        assert!(compute_schedule(&config).is_empty());
    }

    #[tokio::test]
    async fn fire_announcement_publishes_tts_interrupt() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        let mut ann = announcement("09:00");
        ann.chime_file = "assets/chime.wav".to_string();
        let outcome = fire_announcement(&bus, &ann, &defaults());
        assert!(outcome.spoke);
        assert_eq!(outcome.actions, 0);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.topic, topic::INTERRUPT);
        assert_eq!(received.env.r#type, msg::TTS);
        assert_eq!(received.env.payload["content"], "hello");
        assert_eq!(received.env.payload["chime_file"], "assets/chime.wav");
    }

    /// The new BGM fields round-trip into the published payload unchanged
    /// (aside from `chime_file`/`volume`/`bgm_volume` going through the
    /// `effective_*` resolvers), so `npc-speech` can play background music
    /// alongside the chime and speech.
    #[tokio::test]
    async fn fire_announcement_publishes_bgm_fields() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        let mut ann = announcement("09:00");
        ann.bgm_file = "assets/bgm.mp3".to_string();
        ann.bgm_volume = 0.5;
        ann.bgm_play_full = true;
        ann.bgm_end_time = "1:30".to_string();

        let outcome = fire_announcement(&bus, &ann, &defaults());
        assert!(outcome.spoke);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.env.payload["bgm_file"], "assets/bgm.mp3");
        assert_eq!(received.env.payload["bgm_volume"], 0.5);
        assert_eq!(received.env.payload["bgm_play_full"], true);
        assert_eq!(received.env.payload["bgm_end_time"], "1:30");
    }

    /// Fields left at their "unset" sentinel (empty chime/`<= 0.0` volumes)
    /// fall back to `scheduler.defaults` in the published payload — mirrors
    /// how `compute_schedule`/`run` source `defaults` from the same config
    /// snapshot as `entries`.
    #[tokio::test]
    async fn fire_announcement_falls_back_to_scheduler_defaults() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        let mut ann = announcement("09:00");
        ann.chime_file = String::new();
        ann.volume = 0.0;
        ann.bgm_volume = 0.0;

        let scheduler_defaults = SchedulerDefaults {
            chime_file: "assets/default_chime.wav".to_string(),
            volume: 0.8,
            bgm_volume: 0.3,
        };

        fire_announcement(&bus, &ann, &scheduler_defaults);

        // `volume`/`bgm_volume` are `f32` in `AnnouncementConfig`/
        // `SchedulerDefaults`; `serde_json` widens them to `f64` on the way
        // into the payload, so compare against the same widened value
        // rather than the `f64` literal (which differs in its low bits).
        let received = rx.recv().await.unwrap();
        assert_eq!(received.env.payload["chime_file"], "assets/default_chime.wav");
        assert_eq!(received.env.payload["volume"], 0.8_f32 as f64);
        assert_eq!(received.env.payload["bgm_volume"], 0.3_f32 as f64);
    }

    #[tokio::test]
    async fn fire_announcement_skips_empty_text() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        let mut ann = announcement("09:00");
        ann.text = String::new();
        assert!(!fire_announcement(&bus, &ann, &defaults()).fired());
        assert!(rx.try_recv().is_err());
    }

    /// The Go original's text-less `redis_actions` entries: no speech, but
    /// the actions still fire.
    #[tokio::test]
    async fn fire_announcement_runs_actions_without_text() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        let mut ann = announcement("17:00");
        ann.text = String::new();
        ann.actions = vec![ScheduledAction::Action {
            content: "原点に移動して".to_string(),
        }];

        let outcome = fire_announcement(&bus, &ann, &defaults());
        assert!(!outcome.spoke);
        assert_eq!(outcome.actions, 1);
        assert!(outcome.fired());

        let received = rx.recv().await.unwrap();
        assert_eq!(received.topic, topic::ACTION);
        assert_eq!(received.env.r#type, msg::ACTION);
        assert_eq!(received.env.payload["content"], "原点に移動して");
    }

    #[tokio::test]
    async fn fire_announcement_speaks_then_runs_actions_in_order() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        let mut ann = announcement("17:00");
        ann.actions = vec![
            ScheduledAction::Command {
                text: "route patrol".to_string(),
            },
            ScheduledAction::Resume,
        ];
        assert_eq!(fire_announcement(&bus, &ann, &defaults()).actions, 2);

        let first = rx.recv().await.unwrap();
        assert_eq!(first.env.r#type, msg::TTS);
        let second = rx.recv().await.unwrap();
        assert_eq!(second.topic, topic::ACTION);
        assert_eq!(second.env.r#type, "command");
        assert_eq!(second.env.payload["text"], "route patrol");
        let third = rx.recv().await.unwrap();
        assert_eq!(third.topic, topic::INTERRUPT);
        assert_eq!(third.env.r#type, msg::RESUME);
    }

    #[tokio::test]
    async fn raw_action_publishes_verbatim() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        assert!(execute_action(
            &bus,
            &ScheduledAction::Raw {
                topic: "agent:interrupt".to_string(),
                msg_type: "resume".to_string(),
                payload: json!({"why": "test"}),
            }
        ));

        let received = rx.recv().await.unwrap();
        assert_eq!(received.topic, "agent:interrupt");
        assert_eq!(received.env.r#type, "resume");
        assert_eq!(received.env.payload["why"], "test");
    }

    #[tokio::test]
    async fn blank_actions_publish_nothing() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        assert!(!execute_action(&bus, &ScheduledAction::Action { content: "  ".into() }));
        assert!(!execute_action(&bus, &ScheduledAction::Command { text: String::new() }));
        assert!(!execute_action(&bus, &ScheduledAction::Chat { content: String::new() }));
        assert!(!execute_action(
            &bus,
            &ScheduledAction::Speak {
                content: String::new(),
                chime_file: String::new(),
            }
        ));
        assert!(!execute_action(
            &bus,
            &ScheduledAction::Raw {
                topic: String::new(),
                msg_type: "resume".to_string(),
                payload: json!({}),
            }
        ));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn chat_action_injects_a_speech_sense_message() {
        let bus = npc_core::bus::Bus::new();
        let mut rx = bus.subscribe();

        assert!(execute_action(
            &bus,
            &ScheduledAction::Chat {
                content: "今日の予定は?".to_string(),
            }
        ));

        let received = rx.recv().await.unwrap();
        assert_eq!(received.topic, topic::SENSE);
        assert_eq!(received.env.r#type, msg::SPEECH);
        assert_eq!(received.env.payload["content"], "今日の予定は?");
    }

    /// Simulates the `CONFIG_UPDATED` reload path in `run`: a schedule
    /// computed from one config is fully replaced (not merged) by
    /// recomputing from a second, matching the "next lookup sees new data"
    /// contract this hot-reload was built for.
    #[test]
    fn compute_schedule_recomputes_fully_on_new_config() {
        let mut first = Config::default();
        first.scheduler.enabled = true;
        first.scheduler.announcements = vec![announcement("09:00")];
        let entries = compute_schedule(&first);
        assert_eq!(entries.len(), 1);

        let mut second = Config::default();
        second.scheduler.enabled = true;
        second.scheduler.announcements = vec![announcement("10:00"), announcement("11:00")];
        let entries = compute_schedule(&second);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].config.time, "10:00");
        assert_eq!(entries[1].config.time, "11:00");
    }
}
