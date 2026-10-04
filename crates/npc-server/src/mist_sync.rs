//! Serialized, latest-save-wins external registration reconciliation.
use npc_core::{
    mist::{registration, MistClient},
    Config,
};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncStatus {
    pub pending: bool,
    pub applied: bool,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub updated_at: Option<String>,
    pub generation: u64,
}
#[derive(Clone, Default)]
pub struct MistSync {
    status: Arc<Mutex<SyncStatus>>,
    tx: Option<watch::Sender<(u64, Arc<Config>)>>,
}
impl MistSync {
    pub fn start(config: Arc<Config>, shutdown: CancellationToken) -> Self {
        let (tx, mut rx) = watch::channel((1, config));
        let service = Self {
            tx: Some(tx),
            status: Arc::new(Mutex::new(SyncStatus {
                pending: true,
                generation: 1,
                ..Default::default()
            })),
        };
        let status = service.status.clone();
        tokio::spawn(async move {
            loop {
                let (generation, config) = rx.borrow_and_update().clone();
                let result = tokio::select! {
                    _ = shutdown.cancelled() => return,
                    result = sync(&config) => result,
                };
                {
                    let mut state = status.lock().unwrap();
                    if state.generation == generation {
                        state.pending = false;
                        state.updated_at = Some(chrono::Utc::now().to_rfc3339());
                        match result {
                            Ok((applied, warnings)) => {
                                state.applied = applied;
                                state.error = None;
                                state.warnings = warnings;
                            }
                            Err(error) => {
                                let message = format!("{error:#}");
                                tracing::warn!(error = %message, "mistl external registration failed");
                                state.applied = false;
                                state.error = Some(message);
                                state.warnings.clear();
                            }
                        }
                    }
                }
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    changed = rx.changed() => if changed.is_err() { return; },
                }
            }
        });
        service
    }
    pub fn submit(&self, config: Arc<Config>) {
        if let Some(tx) = &self.tx {
            let mut state = self.status.lock().unwrap();
            state.generation += 1;
            state.pending = true;
            tx.send_replace((state.generation, config));
        }
    }
    pub fn status(&self) -> SyncStatus {
        self.status.lock().unwrap().clone()
    }
}
async fn sync(config: &Config) -> anyhow::Result<(bool, Vec<String>)> {
    let payload = registration(config);
    let client = MistClient::new(&config.mist);
    if payload.rooms.is_empty() {
        client.remove().await?;
        Ok((false, vec![]))
    } else {
        let result = client.apply(&payload).await?;
        Ok((true, result.warnings))
    }
}
