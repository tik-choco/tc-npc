//! Scheduler module (ports Go `agent-scheduler`, see
//! `internal/notifier/scheduler.go` in the original service).
//!
//! Reads `config.scheduler.announcements` (`{time, text, chime_file,
//! volume}`, `time` = `"HH:MM"` or `"HH:MM:SS"`, daily recurrence — the Go
//! original turned these into 6-field cron expressions via `robfig/cron`;
//! here we hand-roll the same "next local-time occurrence" semantics with
//! `chrono` instead of pulling in a cron crate). Unlike the Go service this
//! module does **not** play audio itself: firing an announcement just
//! publishes a `tts` message on `agent:interrupt` and `npc-speech` plays the
//! chime and speaks the text.
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

use npc_core::config::AnnouncementConfig;
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
                            fire(&ctx, &entry.config);
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

fn fire(ctx: &ModuleCtx, ann: &AnnouncementConfig) {
    if ann.text.is_empty() {
        // Empty-text announcements are allowed as no-ops for future action
        // hooks (e.g. triggering something other than TTS at a scheduled
        // time) — nothing to publish yet.
        tracing::debug!(time = %ann.time, "npc-scheduler: no-op announcement (empty text), skipping");
        return;
    }

    tracing::info!(time = %ann.time, text = %ann.text, "npc-scheduler: firing announcement");
    ctx.bus.publish(
        topic::INTERRUPT,
        msg::TTS,
        json!({
            "content": ann.text,
            "chime_file": ann.chime_file,
        }),
    );
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
            chime_file: String::new(),
            volume: 1.0,
        }
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
