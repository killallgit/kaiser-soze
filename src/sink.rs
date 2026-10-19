use crate::live::Live;
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
        Self::open_with(path, None).await
    }

    /// Opens the tap file; each line written is also shown by `live`, after the file write and
    /// without ever waiting for it.
    pub async fn open_with(path: &Path, mut live: Option<Live>) -> std::io::Result<Self> {
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
                if let Some(live) = live.as_mut() {
                    live.show(&record);
                }
            }
        });
        Ok(Self {
            tx,
            dropped: Arc::default(),
        })
    }

    /// Queues a line, or drops and counts it when the queue is full. Returns whether it was queued.
    pub fn write(&self, record: Value) -> bool {
        let queued = self.tx.try_send(record).is_ok();
        if !queued {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queued
    }

    /// Waits for room instead of dropping. Only the tap task calls this, never the forwarding path.
    pub async fn write_reliable(&self, record: Value) -> bool {
        self.tx.send(record).await.is_ok()
    }

    #[cfg(test)]
    pub fn detached(capacity: usize) -> (Self, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Self {
                tx,
                dropped: Arc::default(),
            },
            rx,
        )
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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

    #[tokio::test]
    async fn a_full_queue_refuses_write_but_write_reliable_waits_for_room() {
        // It reports the refusal, then writes the reliable line once the writer drains
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        let sink = Sink::open(&path).await.unwrap();

        let accepted: Vec<bool> = (0..SINK_QUEUE + 100)
            .map(|_| sink.write(json!({"live": true})))
            .collect();
        let written = sink.write_reliable(json!({"reliable": true})).await;

        assert!(accepted.contains(&false));
        assert!(written);
        for _ in 0..50 {
            if std::fs::read_to_string(&path).unwrap_or_default().contains("reliable") {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the reliable line never reached the file");
    }
}
