//! mistl's external registration transport. Never reads or edits mistl config.
use crate::{config::MistConfig, Config, ModelRef};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

pub const OWNER: &str = "tc-npc";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registration {
    pub owner: String,
    pub label: String,
    pub providers: Vec<ExternalProvider>,
    pub rooms: Vec<ExternalRoom>,
}
// No Debug implementation: this payload contains credentials.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalProvider {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalRoom {
    pub room: String,
    pub consume: bool,
    pub provide: bool,
    pub shared: Vec<ModelRef>,
}

/// Disabled rooms contribute nothing. Ref order is retained because the first
/// shared ref wins when two HTTP providers offer the same raw model ID.
pub fn registration(config: &Config) -> Registration {
    let http: BTreeMap<_, _> = config
        .providers
        .iter()
        .filter(|p| {
            p.enabled && (p.base_url.starts_with("http://") || p.base_url.starts_with("https://"))
        })
        .map(|p| (p.id.as_str(), p))
        .collect();
    let mut rooms = BTreeMap::<String, ExternalRoom>::new();
    let mut used = BTreeSet::new();
    for p in config.providers.iter().filter(|p| p.enabled) {
        let Some(room) = p.room() else { continue };
        let entry = rooms
            .entry(room.to_string())
            .or_insert_with(|| ExternalRoom {
                room: room.into(),
                consume: true,
                provide: false,
                shared: vec![],
            });
        entry.provide |= p.provide;
        for reference in &p.shared {
            if !reference.model.trim().is_empty()
                && http.contains_key(reference.provider_id.as_str())
            {
                used.insert(reference.provider_id.as_str());
                if !entry.shared.contains(reference) {
                    entry.shared.push(reference.clone());
                }
            }
        }
    }
    Registration {
        owner: OWNER.into(),
        label: "tc-npc".into(),
        providers: used
            .into_iter()
            .map(|id| {
                let p = http[id];
                ExternalProvider {
                    id: p.id.clone(),
                    label: p.label.clone(),
                    base_url: p.base_url.clone(),
                    api_key: p.api_key.clone(),
                    enabled: true,
                }
            })
            .collect(),
        rooms: rooms.into_values().collect(),
    }
}

/// Percent-encode a room as one path segment, including slash and percent.
pub fn room_base_url(base: &str, room: &str) -> String {
    let mut encoded = String::new();
    for byte in room.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("{}/rooms/{encoded}", base.trim_end_matches('/'))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RoomStatus {
    pub room: String,
    pub joined: bool,
    pub providing: bool,
    pub peers: usize,
    #[serde(default)]
    pub models: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppliedRegistration {
    pub owner: String,
    #[serde(default)]
    pub rooms: Vec<ExternalRoom>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub status: ExternalStatus,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExternalStatus {
    #[serde(default)]
    pub rooms: Vec<RoomStatus>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExternalGet {
    pub registrations: Vec<AppliedRegistration>,
}
#[derive(Deserialize)]
pub struct ApplyResult {
    pub owner: String,
    pub applied: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}
#[derive(Deserialize)]
struct RemoveResult {
    owner: String,
    removed: bool,
}

pub struct MistClient {
    config: MistConfig,
    timeout: Duration,
}
impl MistClient {
    pub fn new(config: &MistConfig) -> Self {
        Self {
            config: config.clone(),
            timeout: Duration::from_secs(10),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        operation: &str,
        payload: Option<Vec<u8>>,
    ) -> Result<T> {
        let secrets: Vec<String> = payload
            .as_ref()
            .and_then(|p| serde_json::from_slice::<Registration>(p).ok())
            .map(|r| {
                r.providers
                    .into_iter()
                    .map(|p| p.api_key)
                    .filter(|key| !key.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let work = async {
            let mut command = self.command(PathBuf::from(&self.config.cli_path), operation);
            let child = match command.spawn() {
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && self.config.cli_path == "mistl" =>
                {
                    let local = std::env::var_os("LOCALAPPDATA")
                        .context("mistl not found on PATH; set mist.cli_path")?;
                    self.command(
                        PathBuf::from(local).join("Programs/mistl/mistl.exe"),
                        operation,
                    )
                    .spawn()
                    .context("mistl not found; set mist.cli_path")?
                }
                other => other.context("could not start mistl; check mist.cli_path")?,
            };
            let mut child = child;
            let mut stdin = child.stdin.take().context("mistl stdin unavailable")?;
            // Drain stdout/stderr while writing stdin, so large payloads cannot deadlock.
            let writer = async move {
                if let Some(data) = payload {
                    stdin
                        .write_all(&data)
                        .await
                        .context("writing mistl stdin")?;
                }
                drop(stdin);
                Ok::<_, anyhow::Error>(())
            };
            let ((), output) = tokio::try_join!(writer, async {
                child.wait_with_output().await.context("waiting for mistl")
            })?;
            let mut stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            for key in &secrets {
                stderr = stderr.replace(key, "***");
            }
            anyhow::ensure!(
                output.status.success(),
                "mistl ai external {operation} failed ({}): {stderr}",
                output.status
            );
            // Do not include stdout in parse errors: a faulty CLI might echo keys.
            serde_json::from_slice(&output.stdout)
                .context("mistl returned invalid external API JSON")
        };
        tokio::time::timeout(self.timeout, work)
            .await
            .context("mistl external API timed out")?
    }
    fn command(&self, path: PathBuf, operation: &str) -> Command {
        let mut command = Command::new(path);
        if let Some(instance) = &self.config.instance {
            command.arg("--instance").arg(instance);
        }
        if let Some(dir) = &self.config.state_dir {
            command.arg("--state-dir").arg(dir);
        }
        command.args(["ai", "external", operation, "--owner", OWNER]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        command
    }
    pub async fn apply(&self, payload: &Registration) -> Result<ApplyResult> {
        let result: ApplyResult = self
            .call("apply", Some(serde_json::to_vec(payload)?))
            .await?;
        anyhow::ensure!(
            result.owner == OWNER && result.applied,
            "mistl did not acknowledge tc-npc registration"
        );
        let mut result = result;
        for p in &payload.providers {
            if !p.api_key.is_empty() {
                for warning in &mut result.warnings {
                    *warning = warning.replace(&p.api_key, "***");
                }
            }
        }
        Ok(result)
    }
    pub async fn remove(&self) -> Result<bool> {
        let result: RemoveResult = self.call("remove", None).await?;
        anyhow::ensure!(
            result.owner == OWNER,
            "mistl returned a different registration owner"
        );
        Ok(result.removed)
    }
    pub async fn get(&self) -> Result<ExternalGet> {
        let mut result: ExternalGet = self.call("get", None).await?;
        result.registrations.retain(|r| r.owner == OWNER);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderConfig;
    #[test]
    fn builds_only_usable_shares_and_preserves_resolution_order() {
        let mut config: Config = serde_json::from_value(serde_json::json!({"providers":[
            {"id":"z","base_url":"http://z","api_key":"secret"},
            {"id":"a","base_url":"https://a"},
            {"id":"off","base_url":"http://off","enabled":false},
            {"id":"unused","base_url":"http://unused"},
            {"id":"r","base_url":"mist-network://z-room","provide":true,"shared":[
                {"provider_id":"z","model":"same"},{"provider_id":"a","model":"same"},
                {"provider_id":"off","model":"bad"},{"provider_id":"r","model":"loop"},
                {"provider_id":"missing","model":"bad"},{"provider_id":"z","model":"same"}]},
            {"id":"consumer","base_url":"mist-network://a-room"},
            {"id":"disabled-room","base_url":"mist-network://off-room","enabled":false,"provide":true}
        ]})).unwrap();
        let built = registration(&config);
        assert_eq!(
            built
                .providers
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(built.providers[1].api_key, "secret");
        assert_eq!(
            built
                .rooms
                .iter()
                .map(|r| r.room.as_str())
                .collect::<Vec<_>>(),
            ["a-room", "z-room"]
        );
        assert!(built.rooms[0].consume && !built.rooms[0].provide);
        assert!(built.rooms[1].provide);
        assert_eq!(
            built.rooms[1]
                .shared
                .iter()
                .map(|r| r.provider_id.as_str())
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
        config.providers.reverse();
        assert!(registration(&config) == built);
        config.providers = vec![ProviderConfig::default()];
        assert!(registration(&config).rooms.is_empty());
    }
    #[test]
    fn encodes_room_segment() {
        assert_eq!(
            room_base_url("http://localhost/v1/", "a/b %日本"),
            "http://localhost/v1/rooms/a%2Fb%20%25%E6%97%A5%E6%9C%AC"
        );
    }
}
