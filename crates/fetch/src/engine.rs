//! Local transfer scheduler shared by the CLI and daemon.
//!
//! This replaces the previous "spawn one task per request" pattern with an
//! explicit, bounded lifecycle.  Protocol adapters remain independent of the
//! scheduler, which is essential for compatible retries and future resume.

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

use crate::download::{DownloadRequest, DownloadResult, Downloader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running,
    Complete,
    Failed,
}

#[derive(Debug, Clone)]
pub struct TransferSnapshot {
    pub id: Uuid,
    pub state: TransferState,
    pub error: Option<String>,
    pub result: Option<DownloadResult>,
}

#[derive(Clone)]
pub struct DownloadEngine {
    downloader: Downloader,
    slots: Arc<Semaphore>,
    transfers: Arc<Mutex<HashMap<Uuid, TransferSnapshot>>>,
}

impl DownloadEngine {
    pub fn new(max_active: usize) -> Self {
        assert!(max_active > 0, "max_active must be positive");
        Self {
            downloader: Downloader::new(),
            slots: Arc::new(Semaphore::new(max_active)),
            transfers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn enqueue(&self, request: DownloadRequest) -> Uuid {
        let id = Uuid::new_v4();
        self.transfers.lock().await.insert(
            id,
            TransferSnapshot {
                id,
                state: TransferState::Queued,
                error: None,
                result: None,
            },
        );
        let engine = self.clone();
        tokio::spawn(async move { engine.run(id, request).await });
        id
    }

    pub async fn status(&self, id: Uuid) -> Option<TransferSnapshot> {
        self.transfers.lock().await.get(&id).cloned()
    }

    async fn run(&self, id: Uuid, request: DownloadRequest) {
        let Ok(_permit) = self.slots.clone().acquire_owned().await else {
            self.finish(id, Err("scheduler shut down".to_owned())).await;
            return;
        };
        if let Some(transfer) = self.transfers.lock().await.get_mut(&id) {
            transfer.state = TransferState::Running;
        }
        let result = self.downloader.download(request).await;
        self.finish(id, result.map_err(|error| error.to_string()))
            .await;
    }

    async fn finish(&self, id: Uuid, result: Result<DownloadResult, String>) {
        let mut transfers = self.transfers.lock().await;
        let Some(transfer) = transfers.get_mut(&id) else {
            return;
        };
        match result {
            Ok(result) => {
                transfer.state = TransferState::Complete;
                transfer.result = Some(result);
            }
            Err(error) => {
                transfer.state = TransferState::Failed;
                transfer.error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enqueue_starts_as_queued_or_running() {
        let engine = DownloadEngine::new(1);
        let id = engine
            .enqueue(DownloadRequest {
                sources: vec![url::Url::parse("https://example.test/file").unwrap()],
                output: std::env::temp_dir().join("fetch-engine-test.bin"),
                splits: 1,
                max_connections_per_server: 1,
                sha256: None,
            })
            .await;
        let status = engine.status(id).await.unwrap();
        assert!(matches!(
            status.state,
            TransferState::Queued | TransferState::Running
        ));
    }
}
