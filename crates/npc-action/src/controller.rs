//! Timed movement primitives (ports Go `internal/osc/controller.go`'s
//! `Controller`): set an axis, hold for a duration, zero it. Unlike the Go
//! original — which used a blocking `time.Sleep` with no way to interrupt —
//! every hold here is cancellable mid-sleep via a `CancellationToken`,
//! checked with `tokio::select!` so a `stop` command (or shutdown) can cut a
//! long `fw`/`tr`/`jump` short instead of waiting it out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::actuator::{Actuator, MAX_AXIS_VALUE, MIN_AXIS_VALUE};

/// `JumpPressDuration` in the Go original.
const JUMP_PRESS_DURATION_MS: u64 = 100;

/// Converts a (possibly negative/zero) second count into a `Duration`,
/// matching Go's `time.Duration(seconds * float64(time.Second))` for the
/// non-negative case (negative seconds — which shouldn't occur given the
/// speed-model math always feeds `abs()` values in — clamp to zero).
pub fn secs(seconds: f64) -> Duration {
    if seconds <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(seconds)
    }
}

/// Sleeps for `dur`, unless `cancel` fires first. Returns `(completed,
/// elapsed)`: `completed` is `false` if cancellation cut the sleep short,
/// and `elapsed` is how much of `dur` actually passed (used by the
/// navigator to prorate dead-reckoning for a cancelled move/turn).
pub async fn sleep_cancellable(dur: Duration, cancel: &CancellationToken) -> (bool, Duration) {
    if dur.is_zero() {
        return (true, Duration::ZERO);
    }
    let start = Instant::now();
    tokio::select! {
        _ = cancel.cancelled() => (false, start.elapsed().min(dur)),
        _ = sleep(dur) => (true, dur),
    }
}

pub struct Controller {
    client: Arc<dyn Actuator>,
}

impl Controller {
    pub fn new(client: Arc<dyn Actuator>) -> Self {
        Self { client }
    }

    pub async fn move_forward(&self, seconds: f64, cancel: &CancellationToken) -> anyhow::Result<bool> {
        self.client.vertical(MAX_AXIS_VALUE).await?;
        let (completed, _) = sleep_cancellable(secs(seconds), cancel).await;
        self.client.vertical(0.0).await?;
        Ok(completed)
    }

    /// Ports `Controller.MoveBackward`. Like the Go original, no registered
    /// command actually calls this (the `s` command drives `Vertical`
    /// directly instead — see `dispatcher.rs`); kept for fidelity.
    #[allow(dead_code)]
    pub async fn move_backward(&self, seconds: f64, cancel: &CancellationToken) -> anyhow::Result<bool> {
        self.client.vertical(MIN_AXIS_VALUE).await?;
        let (completed, _) = sleep_cancellable(secs(seconds), cancel).await;
        self.client.vertical(0.0).await?;
        Ok(completed)
    }

    pub async fn turn(&self, intensity: f32, seconds: f64, cancel: &CancellationToken) -> anyhow::Result<bool> {
        self.client.look_horizontal(intensity).await?;
        let (completed, _) = sleep_cancellable(secs(seconds), cancel).await;
        self.client.look_horizontal(0.0).await?;
        Ok(completed)
    }

    /// Ports `Controller.MoveAndTurn`. Not wired to any CLI/NL command (the
    /// Go original didn't expose it either), kept for fidelity.
    #[allow(dead_code)]
    pub async fn move_and_turn(
        &self,
        vertical: f32,
        horizontal: f32,
        turn: f32,
        seconds: f64,
        cancel: &CancellationToken,
    ) -> anyhow::Result<bool> {
        self.client.vertical(vertical).await?;
        self.client.horizontal(horizontal).await?;
        self.client.look_horizontal(turn).await?;
        let (completed, _) = sleep_cancellable(secs(seconds), cancel).await;
        self.client.vertical(0.0).await?;
        self.client.horizontal(0.0).await?;
        self.client.look_horizontal(0.0).await?;
        Ok(completed)
    }

    pub async fn jump(&self, cancel: &CancellationToken) -> anyhow::Result<bool> {
        self.client.jump(true).await?;
        let (completed, _) =
            sleep_cancellable(Duration::from_millis(JUMP_PRESS_DURATION_MS), cancel).await;
        self.client.jump(false).await?;
        Ok(completed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::test_support::{Call, RecordingActuator};

    #[tokio::test]
    async fn move_forward_holds_the_axis_then_zeroes_it() {
        let body = Arc::new(RecordingActuator::default());
        let controller = Controller::new(body.clone());

        let completed = controller
            .move_forward(0.0, &CancellationToken::new())
            .await
            .unwrap();

        assert!(completed);
        assert_eq!(
            body.calls(),
            vec![Call::Vertical(MAX_AXIS_VALUE), Call::Vertical(0.0)]
        );
    }

    #[tokio::test]
    async fn a_cancelled_hold_still_zeroes_the_axis() {
        // The axis stays where it was put, so cutting a move short without
        // zeroing would leave the body walking forever. Cancelling before
        // the hold begins is the sharpest version of that case.
        let body = Arc::new(RecordingActuator::default());
        let controller = Controller::new(body.clone());
        let cancel = CancellationToken::new();
        cancel.cancel();

        let completed = controller.move_forward(60.0, &cancel).await.unwrap();

        assert!(!completed, "a cancelled hold reports incomplete");
        assert_eq!(
            body.calls(),
            vec![Call::Vertical(MAX_AXIS_VALUE), Call::Vertical(0.0)],
            "the axis must be released even though the hold was cut short"
        );
    }

    #[tokio::test]
    async fn turn_releases_the_look_axis() {
        let body = Arc::new(RecordingActuator::default());
        let controller = Controller::new(body.clone());

        controller
            .turn(0.75, 0.0, &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(
            body.calls(),
            vec![Call::LookHorizontal(0.75), Call::LookHorizontal(0.0)]
        );
    }

    #[tokio::test]
    async fn jump_presses_then_releases() {
        let body = Arc::new(RecordingActuator::default());
        let controller = Controller::new(body.clone());

        controller.jump(&CancellationToken::new()).await.unwrap();

        assert_eq!(body.calls(), vec![Call::Jump(true), Call::Jump(false)]);
    }
}
