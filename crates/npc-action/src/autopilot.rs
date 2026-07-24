//! High-level "go to a point / run a route" behavior on top of
//! [`crate::navigator::Navigator`] (ports Go `internal/osc/autopilot.go`).

use std::sync::Arc;

use npc_core::config::{LocationConfig, RouteConfig};
use tokio_util::sync::CancellationToken;

use crate::controller::{secs, sleep_cancellable};
use crate::navigator::{max_dps, Navigator, PositionData};

const MIN_DISTANCE_THRESHOLD: f64 = 0.01;
const MIN_ROTATION_THRESHOLD: f64 = 0.5;
const MIN_MOVE_TIME: f64 = 0.1;

/// The Go config schema's `Waypoint.Wait` was a `float64` number of seconds
/// to pause at a waypoint. `npc-core::config::WaypointConfig.wait` (owned by
/// another crate, not editable here) narrowed that to a `bool`. This port
/// therefore uses a fixed pause duration whenever a waypoint's `wait` flag
/// is set, rather than a per-waypoint configurable one. See this crate's
/// top-level worker report for the full rationale.
const WAYPOINT_WAIT_SECONDS: f64 = 2.0;

/// Ports Go `osc.Autopilot`.
pub struct Autopilot {
    nav: Arc<Navigator>,
}

impl Autopilot {
    pub fn new(nav: Arc<Navigator>) -> Self {
        Self { nav }
    }

    /// Ports `Autopilot.GoTo`: rotate to face the target, then move the
    /// straight-line distance, splitting the `seconds` time budget between
    /// the two (turn gets `|rotation| / maxDPS`, the remainder — floored at
    /// `MIN_MOVE_TIME` — goes to the move).
    pub async fn go_to(
        &self,
        target_x: f64,
        target_y: f64,
        seconds: f64,
        cancel: &CancellationToken,
    ) -> anyhow::Result<bool> {
        let PositionData { x, y, heading } = self.nav.pos.get();

        let dx = target_x - x;
        let dy = target_y - y;
        let distance = (dx * dx + dy * dy).sqrt();

        if distance < MIN_DISTANCE_THRESHOLD {
            return Ok(true);
        }

        let mut target_heading = dx.atan2(dy) * (180.0 / std::f64::consts::PI);
        if target_heading < 0.0 {
            target_heading += 360.0;
        }

        let mut rotation = target_heading - heading;
        if rotation > 180.0 {
            rotation -= 360.0;
        }
        if rotation < -180.0 {
            rotation += 360.0;
        }

        let turn_time = rotation.abs() / max_dps();
        let mut move_time = seconds - turn_time;
        if move_time < MIN_MOVE_TIME {
            move_time = MIN_MOVE_TIME;
        }

        if rotation.abs() > MIN_ROTATION_THRESHOLD {
            let completed = self.nav.turn_degrees(rotation, cancel).await?;
            if !completed {
                return Ok(false);
            }
        }

        self.nav.move_meters_in_time(distance, move_time, cancel).await
    }

    /// Ports `Autopilot.GoToWithHeading`: `go_to`, then a final turn to face
    /// `target_heading` exactly.
    pub async fn go_to_with_heading(
        &self,
        target_x: f64,
        target_y: f64,
        target_heading: f64,
        seconds: f64,
        cancel: &CancellationToken,
    ) -> anyhow::Result<bool> {
        let completed = self.go_to(target_x, target_y, seconds, cancel).await?;
        if !completed {
            return Ok(false);
        }

        let PositionData { heading: current_heading, .. } = self.nav.pos.get();
        let mut rotation = target_heading - current_heading;
        if rotation > 180.0 {
            rotation -= 360.0;
        }
        if rotation < -180.0 {
            rotation += 360.0;
        }

        if rotation.abs() > MIN_ROTATION_THRESHOLD {
            return self.nav.turn_degrees(rotation, cancel).await;
        }
        Ok(true)
    }

    /// Ports `Autopilot.ReturnToOrigin`.
    pub async fn return_to_origin(&self, seconds: f64, cancel: &CancellationToken) -> anyhow::Result<bool> {
        self.go_to(0.0, 0.0, seconds, cancel).await
    }

    /// Ports `Autopilot.RunRoute`: walks `route.waypoints` in order,
    /// resolving each `Waypoint.location` against `locations`
    /// (case-insensitively), looping if `route.loop` is set. `cancel` is
    /// checked before each waypoint and during each move/turn/wait, so a
    /// `stop` cuts the whole route short (matching the Go original's `stop
    /// <-chan struct{}` parameter). `log` receives human-readable progress
    /// lines for the caller to publish (e.g. as `action_log` bus messages).
    pub async fn run_route(
        &self,
        route: &RouteConfig,
        locations: &[LocationConfig],
        cancel: &CancellationToken,
        mut log: impl FnMut(String),
    ) -> anyhow::Result<()> {
        log(format!(
            "Starting route '{}' (loop={}, {} waypoints)",
            route.name,
            route.r#loop,
            route.waypoints.len()
        ));

        let mut lap: u32 = 1;
        loop {
            if route.r#loop {
                log(format!("--- Lap {lap} ---"));
            }

            for (i, wp) in route.waypoints.iter().enumerate() {
                if cancel.is_cancelled() {
                    log("Route stopped".to_string());
                    return Ok(());
                }

                let Some(loc) = find_location(locations, &wp.location) else {
                    anyhow::bail!("location '{}' not found", wp.location);
                };

                log(format!("Waypoint {}/{}: '{}'", i + 1, route.waypoints.len(), loc.name));
                let completed = self
                    .go_to_with_heading(loc.x, loc.y, loc.heading, wp.seconds, cancel)
                    .await?;
                if !completed {
                    log("Route stopped".to_string());
                    return Ok(());
                }
                log(format!("Arrived at '{}' - {}", loc.name, self.nav.pos.get().display()));

                if wp.wait {
                    log(format!("Waiting {WAYPOINT_WAIT_SECONDS:.1}s..."));
                    let (completed, _) = sleep_cancellable(secs(WAYPOINT_WAIT_SECONDS), cancel).await;
                    if !completed {
                        log("Route stopped".to_string());
                        return Ok(());
                    }
                }
            }

            if !route.r#loop {
                break;
            }
            lap += 1;
        }

        log(format!("Route '{}' completed", route.name));
        Ok(())
    }
}

fn find_location<'a>(locations: &'a [LocationConfig], name: &str) -> Option<&'a LocationConfig> {
    locations.iter().find(|l| l.name.to_lowercase() == name.to_lowercase())
}
