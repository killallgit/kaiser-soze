use crate::line::Line;
use crate::live::Live;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

const SINK_QUEUE: usize = 4096;

pub struct Sink<T> {
    tx: mpsc::Sender<T>,
    dropped: Arc<AtomicU64>,
}

impl<T> Clone for Sink<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            dropped: self.dropped.clone(),
        }
    }
}

impl<T: Serialize + Send + 'static> Sink<T> {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_showing(path, |_| {}).await
    }

    /// Opens the file; each item written is one JSON line, then handed to `shown` without waiting for anything.
    /// An item that cannot be serialised is dropped and counted, and the next one is written.
    async fn open_showing(path: &Path, mut shown: impl FnMut(&T) + Send + 'static) -> std::io::Result<Self> {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .await?;
        let (tx, mut rx) = mpsc::channel::<T>(SINK_QUEUE);
        let dropped = Arc::<AtomicU64>::default();
        let unwritable = dropped.clone();
        tokio::spawn(async move {
            while let Some(record) = rx.recv().await {
                let Ok(mut line) = serde_json::to_string(&record) else {
                    unwritable.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                line.push('\n');
                if file.write_all(line.as_bytes()).await.is_err() || file.flush().await.is_err() {
                    break;
                }
                shown(&record);
            }
        });
        Ok(Self { tx, dropped })
    }
}

impl Sink<Line> {
    /// Opens the tap file; each line written is also shown by `live`, after the file write and
    /// without ever waiting for it.
    pub async fn open_with(path: &Path, mut live: Option<Live>) -> std::io::Result<Self> {
        Self::open_showing(path, move |line| {
            if let Some(live) = live.as_mut() {
                live.show(line);
            }
        })
        .await
    }
}

impl<T> Sink<T> {
    /// Queues a line, or drops and counts it when the queue is full. Returns whether it was queued.
    pub fn write(&self, record: T) -> bool {
        let queued = self.tx.try_send(record).is_ok();
        if !queued {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queued
    }

    /// Waits for room instead of dropping. Only the tap task calls this, never the forwarding path.
    pub async fn write_reliable(&self, record: T) -> bool {
        self.tx.send(record).await.is_ok()
    }

    #[cfg(test)]
    pub fn detached(capacity: usize) -> (Self, mpsc::Receiver<T>) {
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
    use serde::ser::Error;
    use serde::Serializer;
    use serde_json::{json, Value};
    use std::os::unix::fs::PermissionsExt;

    async fn wait_for_text(path: &Path, text: &str) {
        for _ in 0..50 {
            if std::fs::read_to_string(path).unwrap_or_default().contains(text) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the line never reached the file");
    }

    #[tokio::test]
    async fn creates_the_tap_file_readable_only_by_its_owner() {
        // It creates a new tap file with mode 0600
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");

        Sink::<Value>::open(&path).await.unwrap();

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
        wait_for_text(&path, "reliable").await;
    }

    enum Item {
        Plain(u8),
        Unserialisable,
    }

    impl Serialize for Item {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                Self::Plain(number) => serializer.serialize_u8(*number),
                Self::Unserialisable => Err(S::Error::custom("synthetic")),
            }
        }
    }

    #[tokio::test]
    async fn an_item_that_cannot_be_serialised_is_dropped_and_counted_and_the_next_is_written() {
        // It keeps writing after the failure, and counts the item as dropped
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        let sink = Sink::open(&path).await.unwrap();

        sink.write_reliable(Item::Plain(1)).await;
        sink.write_reliable(Item::Unserialisable).await;
        sink.write_reliable(Item::Plain(2)).await;

        wait_for_text(&path, "2").await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1\n2\n");
        assert_eq!(sink.dropped(), 1);
    }
}
