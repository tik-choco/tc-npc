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
//! Config hot-reload is intentionally NOT implemented: the schedule is
//! computed once from the `Config` snapshot handed to `run` at startup. A
//! future version could watch `PUT /api/config` (or the config file) and
//! recompute the schedule when `scheduler.announcements` changes.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeZone};
use serde_json::json;

use npc_core::config::AnnouncementConfig;
use npc_core::{msg, topic, Module, ModuleCtx};

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
        let now = Local::now();
        let mut entries: Vec<ScheduledAnnouncement> = ctx
            .config
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
            tracing::info!("npc-scheduler: no valid announcements configured, idling until shutdown");
            ctx.shutdown.cancelled().await;
            return Ok(());
        }

        for entry in &entries {
            tracing::info!(time = %entry.config.time, next = %entry.next, "npc-scheduler: scheduled announcement");
        }

        loop {
            let wake_at = entries
                .iter()
                .map(|e| e.next)
                .min()
                .expect("entries is non-empty");

            let duration = (wake_at - Local::now()).to_std().unwrap_or(Duration::ZERO);

            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-scheduler: shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(duration) => {}
            }

            // Fire every entry due at (or before, to absorb scheduling
            // jitter) `wake_at` — this is what makes multiple announcements
            // configured for the same time fire together.
            for entry in entries.iter_mut() {
                if entry.next <= wake_at {
                    fire(&ctx, &entry.config);
                    entry.next += chrono::Duration::hours(24);
                }
            }
        }
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
}
