use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

const SINK_QUEUE: usize = 4096;

#[derive(Clone)]
pub struct Sink {
    tx: mpsc::Sender<Value>,
    dropped: Arc<AtomicU64>,
}

impl Sink {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .await?;
        let (tx, mut rx) = mpsc::channel::<Value>(SINK_QUEUE);
        tokio::spawn(async move {
            while let Some(record) = rx.recv().await {
                let line = format!("{record}\n");
                if file.write_all(line.as_bytes()).await.is_err() || file.flush().await.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            tx,
            dropped: Arc::default(),
        })
    }

    pub fn write(&self, record: Value) {
        if self.tx.try_send(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn creates_the_tap_file_readable_only_by_its_owner() {
        // It creates a new tap file with mode 0600
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");

        Sink::open(&path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
