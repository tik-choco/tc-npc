//! Minimal VRChat OSC client (ports Go `internal/osc/vrc.go`'s `VRCClient`,
//! trimmed to the input axes/buttons this crate actually drives). Uses
//! `rosc` for message encoding and a plain `tokio::net::UdpSocket` for
//! transport — VRChat's OSC input listens on UDP, fire-and-forget, no
//! response is expected.

use std::net::SocketAddr;

use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;

/// Fallback target when `osc_address` fails to resolve.
const DEFAULT_OSC_ADDRESS: &str = "127.0.0.1:9000";

pub const MAX_AXIS_VALUE: f32 = 1.0;
pub const MIN_AXIS_VALUE: f32 = -1.0;

pub struct VrcClient {
    socket: UdpSocket,
    target: SocketAddr,
}

impl VrcClient {
    /// Resolves `addr` (accepts both `ip:port` and `host:port` forms, unlike
    /// the Go client which only ever used the raw string) and binds an
    /// ephemeral local UDP socket to send from. A resolution failure falls
    /// back to [`DEFAULT_OSC_ADDRESS`], matching the Go client's behavior of
    /// falling back to `DefaultOSCAddress` on a malformed `host:port` pair.
    pub async fn connect(addr: &str) -> anyhow::Result<Self> {
        let resolved = tokio::net::lookup_host(addr).await.ok().and_then(|mut it| it.next());
        let target = match resolved {
            Some(addr) => addr,
            None => {
                tracing::warn!(
                    addr,
                    fallback = DEFAULT_OSC_ADDRESS,
                    "npc-action: could not resolve osc_address, using default"
                );
                DEFAULT_OSC_ADDRESS
                    .parse()
                    .expect("DEFAULT_OSC_ADDRESS is a valid socket address")
            }
        };

        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        Ok(Self { socket, target })
    }

    fn clamp_axis(value: f32) -> f32 {
        if value < MIN_AXIS_VALUE {
            tracing::warn!(value, clamped_to = MIN_AXIS_VALUE, "npc-action: OSC axis value clamped");
            MIN_AXIS_VALUE
        } else if value > MAX_AXIS_VALUE {
            tracing::warn!(value, clamped_to = MAX_AXIS_VALUE, "npc-action: OSC axis value clamped");
            MAX_AXIS_VALUE
        } else {
            value
        }
    }

    async fn send(&self, addr: &str, args: Vec<OscType>) -> anyhow::Result<()> {
        let packet = OscPacket::Message(OscMessage { addr: addr.to_string(), args });
        let bytes = rosc::encoder::encode(&packet)
            .map_err(|e| anyhow::anyhow!("failed to encode OSC message {addr}: {e:?}"))?;
        self.socket.send_to(&bytes, self.target).await?;
        Ok(())
    }

    async fn send_axis(&self, addr: &str, value: f32) -> anyhow::Result<()> {
        self.send(addr, vec![OscType::Float(Self::clamp_axis(value))]).await
    }

    async fn send_button(&self, addr: &str, pressed: bool) -> anyhow::Result<()> {
        self.send(addr, vec![OscType::Int(if pressed { 1 } else { 0 })]).await
    }

    pub async fn vertical(&self, value: f32) -> anyhow::Result<()> {
        self.send_axis("/input/Vertical", value).await
    }

    pub async fn horizontal(&self, value: f32) -> anyhow::Result<()> {
        self.send_axis("/input/Horizontal", value).await
    }

    pub async fn look_horizontal(&self, value: f32) -> anyhow::Result<()> {
        self.send_axis("/input/LookHorizontal", value).await
    }

    pub async fn jump(&self, pressed: bool) -> anyhow::Result<()> {
        self.send_button("/input/Jump", pressed).await
    }
}
