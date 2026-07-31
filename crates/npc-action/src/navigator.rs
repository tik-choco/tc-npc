//! Dead-reckoned position tracking and the intensity/speed model (ports Go
//! `internal/osc/position.go` + `internal/osc/navigator.go`). The speed
//! model constants and `intensityForDPS`/`intensityForMPS` solvers are
//! ported byte-for-byte; see the unit tests at the bottom for hand-computed
//! checks against the Go values.

use std::f64::consts::PI;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use npc_core::Bus;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::controller::{secs, sleep_cancellable};
use crate::actuator::{Actuator, MAX_AXIS_VALUE, MIN_AXIS_VALUE};

// --- Speed model constants (ports Go `navigator.go`'s consts, unchanged) ---

pub const TURN_DPS_SLOPE: f64 = 400.0;
pub const TURN_DPS_OFFSET: f64 = 0.5;
pub const MIN_TURN_INTENSITY: f32 = 0.6;

pub const MOVE_MPS_SLOPE: f64 = 2.25;
pub const MOVE_MPS_OFFSET: f64 = 0.1;
pub const MIN_MOVE_INTENSITY: f32 = 0.25;

/// Ports `intensityForDPS`: solves the linear turn-speed model
/// `dps = TURN_DPS_SLOPE * (intensity - TURN_DPS_OFFSET)` for `intensity`,
/// clamped to `[MIN_TURN_INTENSITY, MAX_AXIS_VALUE]`.
pub fn intensity_for_dps(target_dps: f64) -> f32 {
    let mut intensity = (target_dps / TURN_DPS_SLOPE + TURN_DPS_OFFSET) as f32;
    if intensity < MIN_TURN_INTENSITY {
        intensity = MIN_TURN_INTENSITY;
    }
    if intensity > MAX_AXIS_VALUE {
        intensity = MAX_AXIS_VALUE;
    }
    intensity
}

/// Ports `intensityForMPS`: same shape as [`intensity_for_dps`] for the
/// move-speed model.
pub fn intensity_for_mps(target_mps: f64) -> f32 {
    let mut intensity = (target_mps / MOVE_MPS_SLOPE + MOVE_MPS_OFFSET) as f32;
    if intensity < MIN_MOVE_INTENSITY {
        intensity = MIN_MOVE_INTENSITY;
    }
    if intensity > MAX_AXIS_VALUE {
        intensity = MAX_AXIS_VALUE;
    }
    intensity
}

/// Ports `maxMPS`.
pub fn max_mps() -> f64 {
    MOVE_MPS_SLOPE * (MAX_AXIS_VALUE as f64 - MOVE_MPS_OFFSET)
}

/// Ports `maxDPS`.
pub fn max_dps() -> f64 {
    TURN_DPS_SLOPE * (MAX_AXIS_VALUE as f64 - TURN_DPS_OFFSET)
}

/// Snapshot of dead-reckoned pose. Serialized directly as the `npc:ui`
/// `position` bus payload (`{x, y, heading}`).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct PositionData {
    pub x: f64,
    pub y: f64,
    pub heading: f64,
}

impl PositionData {
    /// Ports `Position.String`.
    pub fn display(&self) -> String {
        format!("X={:.2} Y={:.2} Heading={:.1}\u{b0}", self.x, self.y, self.heading)
    }
}

/// Ports `osc.Position`. Interior mutability so it can be shared/read (e.g.
/// by the `pos` command) while movement is in progress.
pub struct Position(Mutex<PositionData>);

impl Position {
    pub fn new() -> Self {
        Self(Mutex::new(PositionData::default()))
    }

    pub fn reset(&self) {
        *self.0.lock().unwrap() = PositionData::default();
    }

    /// Ports `Position.AddMove`: advances `(x, y)` along the current
    /// heading by `meters` (heading 0 = +Y, 90 = +X, matching the Go
    /// `math.Sin`/`math.Cos` convention).
    pub fn add_move(&self, meters: f64) {
        let mut p = self.0.lock().unwrap();
        let rad = p.heading * PI / 180.0;
        p.x += meters * rad.sin();
        p.y += meters * rad.cos();
    }

    /// Ports `Position.AddRotation`: wraps heading into `[0, 360)`.
    pub fn add_rotation(&self, degrees: f64) {
        let mut p = self.0.lock().unwrap();
        let mut heading = (p.heading + degrees) % 360.0;
        if heading < 0.0 {
            heading += 360.0;
        }
        p.heading = heading;
    }

    pub fn get(&self) -> PositionData {
        *self.0.lock().unwrap()
    }
}

impl Default for Position {
    fn default() -> Self {
        Self::new()
    }
}

/// Ports Go `osc.Navigator`.
pub struct Navigator {
    client: Arc<dyn Actuator>,
    pub pos: Position,
    bus: Bus,
}

impl Navigator {
    pub fn new(client: Arc<dyn Actuator>, bus: Bus) -> Self {
        Self { client, pos: Position::new(), bus }
    }

    /// Publishes the current pose as `npc:ui` / `position`. Called after
    /// every dead-reckoning update (see the movement methods below) plus
    /// explicitly by the `reset` command.
    pub fn publish_position(&self) {
        self.bus.publish("npc:ui", "position", self.pos.get());
    }

    /// Applies a (possibly partial, if a move was cancelled mid-flight)
    /// fraction of an intended move/turn and republishes position. Full
    /// (non-cancelled) moves always use `fraction == 1.0`, exactly matching
    /// the Go original, which always applied the full intended delta since
    /// it had no way to cancel mid-sleep.
    fn apply_move(&self, meters: f64, fraction: f64) {
        self.pos.add_move(meters * fraction.clamp(0.0, 1.0));
        self.publish_position();
    }

    fn apply_rotation(&self, degrees: f64, fraction: f64) {
        self.pos.add_rotation(degrees * fraction.clamp(0.0, 1.0));
        self.publish_position();
    }

    /// Ports `Navigator.MoveMeters`.
    pub async fn move_meters(&self, meters: f64, cancel: &CancellationToken) -> anyhow::Result<bool> {
        let seconds = meters.abs() / max_mps();
        let intensity = if meters < 0.0 { MIN_AXIS_VALUE } else { MAX_AXIS_VALUE };
        self.client.vertical(intensity).await?;
        let dur = secs(seconds);
        let (completed, elapsed) = sleep_cancellable(dur, cancel).await;
        self.client.vertical(0.0).await?;
        self.apply_move(meters, fraction_of(elapsed, dur));
        Ok(completed)
    }

    /// Ports `Navigator.TurnDegrees`.
    pub async fn turn_degrees(&self, degrees: f64, cancel: &CancellationToken) -> anyhow::Result<bool> {
        let seconds = degrees.abs() / max_dps();
        let intensity = if degrees < 0.0 { MIN_AXIS_VALUE } else { MAX_AXIS_VALUE };
        self.client.look_horizontal(intensity).await?;
        let dur = secs(seconds);
        let (completed, elapsed) = sleep_cancellable(dur, cancel).await;
        self.client.look_horizontal(0.0).await?;
        self.apply_rotation(degrees, fraction_of(elapsed, dur));
        Ok(completed)
    }

    /// Ports `Navigator.MoveMetersInTime`.
    pub async fn move_meters_in_time(
        &self,
        meters: f64,
        seconds: f64,
        cancel: &CancellationToken,
    ) -> anyhow::Result<bool> {
        if seconds <= 0.0 {
            return Ok(true);
        }
        let target_mps = meters.abs() / seconds;
        let mut intensity = intensity_for_mps(target_mps);
        let actual_mps = MOVE_MPS_SLOPE * (intensity as f64 - MOVE_MPS_OFFSET);
        let actual_seconds = meters.abs() / actual_mps;
        if meters < 0.0 {
            intensity = -intensity;
        }
        self.client.vertical(intensity).await?;
        let dur = secs(actual_seconds);
        let (completed, elapsed) = sleep_cancellable(dur, cancel).await;
        self.client.vertical(0.0).await?;
        self.apply_move(meters, fraction_of(elapsed, dur));
        Ok(completed)
    }

    /// Ports `Navigator.TurnDegreesInTime`.
    pub async fn turn_degrees_in_time(
        &self,
        degrees: f64,
        seconds: f64,
        cancel: &CancellationToken,
    ) -> anyhow::Result<bool> {
        if seconds <= 0.0 {
            return Ok(true);
        }
        let target_dps = degrees.abs() / seconds;
        let mut intensity = intensity_for_dps(target_dps);
        let actual_dps = TURN_DPS_SLOPE * (intensity as f64 - TURN_DPS_OFFSET);
        let actual_seconds = degrees.abs() / actual_dps;
        if degrees < 0.0 {
            intensity = -intensity;
        }
        self.client.look_horizontal(intensity).await?;
        let dur = secs(actual_seconds);
        let (completed, elapsed) = sleep_cancellable(dur, cancel).await;
        self.client.look_horizontal(0.0).await?;
        self.apply_rotation(degrees, fraction_of(elapsed, dur));
        Ok(completed)
    }
}

fn fraction_of(elapsed: Duration, total: Duration) -> f64 {
    if total.is_zero() {
        1.0
    } else {
        (elapsed.as_secs_f64() / total.as_secs_f64()).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    #[test]
    fn turn_intensity_solves_linear_model() {
        // 200 dps -> 200/400 + 0.5 = 1.0 (right at the axis ceiling).
        assert!((intensity_for_dps(200.0) - 1.0).abs() < 1e-6);
        // 100 dps -> 100/400 + 0.5 = 0.75.
        assert!((intensity_for_dps(100.0) - 0.75).abs() < 1e-6);
        // 0 dps -> 0.5, below MIN_TURN_INTENSITY (0.6) -> clamped up.
        assert!((intensity_for_dps(0.0) - 0.6).abs() < 1e-6);
        // Absurdly high dps clamps to MAX_AXIS_VALUE (1.0).
        assert!((intensity_for_dps(10_000.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn move_intensity_solves_linear_model() {
        // 2.0 mps -> 2/2.25 + 0.1 = 0.98888...
        assert!((intensity_for_mps(2.0) - (2.0 / 2.25 + 0.1) as f32).abs() < 1e-6);
        // 0 mps -> 0.1, below MIN_MOVE_INTENSITY (0.25) -> clamped up.
        assert!((intensity_for_mps(0.0) - 0.25).abs() < 1e-6);
        // Absurdly high mps clamps to MAX_AXIS_VALUE (1.0).
        assert!((intensity_for_mps(10_000.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn max_speeds_match_hand_computation() {
        // maxDPS = 400 * (1 - 0.5) = 200.
        assert!((max_dps() - 200.0).abs() < EPS);
        // maxMPS = 2.25 * (1 - 0.1) = 2.025.
        assert!((max_mps() - 2.025).abs() < EPS);
    }

    #[test]
    fn dead_reckoning_add_move_along_heading() {
        let pos = Position::new();
        // Heading 0 (facing +Y): moving 10m adds to Y only.
        pos.add_move(10.0);
        let p = pos.get();
        assert!((p.x - 0.0).abs() < EPS);
        assert!((p.y - 10.0).abs() < EPS);

        // Rotate to heading 90 (facing +X): moving 5m adds to X only.
        pos.add_rotation(90.0);
        pos.add_move(5.0);
        let p = pos.get();
        assert!((p.x - 5.0).abs() < 1e-9);
        assert!((p.y - 10.0).abs() < 1e-9);
        assert!((p.heading - 90.0).abs() < EPS);
    }

    #[test]
    fn dead_reckoning_rotation_wraps_into_0_360() {
        let pos = Position::new();
        pos.add_rotation(370.0);
        assert!((pos.get().heading - 10.0).abs() < EPS);

        pos.reset();
        pos.add_rotation(90.0);
        pos.add_rotation(-100.0);
        // 90 - 100 = -10 -> wraps to 350, matching Go's math.Mod + correction.
        assert!((pos.get().heading - 350.0).abs() < EPS);
    }

    #[test]
    fn reset_clears_position() {
        let pos = Position::new();
        pos.add_move(5.0);
        pos.add_rotation(45.0);
        pos.reset();
        let p = pos.get();
        assert_eq!(p.x, 0.0);
        assert_eq!(p.y, 0.0);
        assert_eq!(p.heading, 0.0);
    }
}
