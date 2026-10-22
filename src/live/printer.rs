use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

pub(super) const PRINT_QUEUE: usize = 256;

pub struct Printer {
    tx: mpsc::Sender<String>,
    dropped: Arc<AtomicU64>,
}

impl Printer {
    pub fn spawn<W: AsyncWrite + Send + Unpin + 'static>(mut writer: W) -> Self {
        let (tx, mut rx) = mpsc::channel::<String>(PRINT_QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let counted = dropped.clone();
        tokio::spawn(async move {
            let mut reported = 0;
            while let Some(line) = rx.recv().await {
                let total = counted.load(Ordering::Relaxed);
                let mut out = String::new();
                if total > reported {
                    out = format!("({total} live lines dropped)\n");
                    reported = total;
                }
                out.push_str(&line);
                out.push('\n');
                if writer.write_all(out.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
        });
        Self { tx, dropped }
    }

    /// Queues a line, or drops and counts it when the queue is full. Never waits.
    pub fn offer(&self, line: String) {
        if self.tx.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
