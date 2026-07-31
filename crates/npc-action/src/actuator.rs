//! The actuator seam: what "the body" can be told to do, independent of
//! what is on the other end of it.
//!
//! Everything above this trait — [`Controller`](crate::controller)'s timed
//! primitives, [`Navigator`](crate::navigator)'s dead-reckoned
//! metres/degrees, [`Autopilot`](crate::autopilot)'s routes, and the
//! dispatcher's CLI commands — is written against these four calls and
//! knows nothing about VRChat or OSC. [`VrcClient`](crate::osc::VrcClient)
//! is the implementation today; **a different embodiment is a second
//! implementor wired in at [`crate::module`], not a fork of this crate.**
//!
//! The surface is deliberately the gamepad-shaped one the Go original drove
//! VRChat with — two translation axes, one rotation axis, one button —
//! because that is also the shape a velocity-commanded body takes: an
//! implementation is free to read `vertical` as linear velocity and
//! `look_horizontal` as angular velocity.
//!
//! ## Contract
//!
//! - Axis values are `MIN_AXIS_VALUE..=MAX_AXIS_VALUE`. An implementation
//!   **clamps** out-of-range values rather than erroring: the speed model
//!   upstream computes axis intensities in floating point and can land a
//!   hair past the bound, and refusing the move would be the worse answer.
//! - Every call is fire-and-forget in spirit: returning `Ok(())` means the
//!   command was handed to the body, not that the body has finished moving.
//!   Duration is the caller's business ([`Controller`](crate::controller)
//!   holds an axis and then zeroes it).
//! - An axis stays where it was put until it is set again, so a caller that
//!   sets a non-zero axis is responsible for zeroing it — including on the
//!   cancellation path, or the body keeps walking after the command that
//!   started it was cut short.

use async_trait::async_trait;

/// Upper bound of an axis (full forward / right).
pub const MAX_AXIS_VALUE: f32 = 1.0;
/// Lower bound of an axis (full backward / left).
pub const MIN_AXIS_VALUE: f32 = -1.0;

/// A body this module can drive. See the module docs for the contract.
#[async_trait]
pub trait Actuator: Send + Sync {
    /// Forward/backward translation; `MAX_AXIS_VALUE` is full forward.
    async fn vertical(&self, value: f32) -> anyhow::Result<()>;

    /// Sideways translation (strafe); `MAX_AXIS_VALUE` is full right.
    async fn horizontal(&self, value: f32) -> anyhow::Result<()>;

    /// Yaw; `MAX_AXIS_VALUE` is turning right as fast as the body turns.
    async fn look_horizontal(&self, value: f32) -> anyhow::Result<()>;

    /// Jump control, held down for as long as `pressed` is true. A body
    /// with nothing to jump with may implement this as a no-op `Ok(())`.
    async fn jump(&self, pressed: bool) -> anyhow::Result<()>;
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A body that only remembers what it was told. Before the trait
    //! existed, driving an axis meant opening a real UDP socket, so the
    //! layers above [`Actuator`] were only ever tested for their arithmetic
    //! — never for what they actually sent, or for whether they put an axis
    //! back to zero afterwards.

    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) enum Call {
        Vertical(f32),
        Horizontal(f32),
        LookHorizontal(f32),
        Jump(bool),
    }

    #[derive(Default)]
    pub(crate) struct RecordingActuator {
        calls: Mutex<Vec<Call>>,
    }

    impl RecordingActuator {
        /// Every call so far, in the order they were made.
        pub(crate) fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("recording actuator lock").clone()
        }

        fn record(&self, call: Call) {
            self.calls.lock().expect("recording actuator lock").push(call);
        }
    }

    #[async_trait]
    impl Actuator for RecordingActuator {
        async fn vertical(&self, value: f32) -> anyhow::Result<()> {
            self.record(Call::Vertical(value));
            Ok(())
        }

        async fn horizontal(&self, value: f32) -> anyhow::Result<()> {
            self.record(Call::Horizontal(value));
            Ok(())
        }

        async fn look_horizontal(&self, value: f32) -> anyhow::Result<()> {
            self.record(Call::LookHorizontal(value));
            Ok(())
        }

        async fn jump(&self, pressed: bool) -> anyhow::Result<()> {
            self.record(Call::Jump(pressed));
            Ok(())
        }
    }
}
