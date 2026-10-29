use crate::logging::{Fields, Level, Logger};
use serde::Serialize;
use std::io::SeekFrom;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
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
    #[cfg(test)]
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_reporting(path, Logger::default()).await
    }

    pub async fn open_reporting(path: &Path, log: Logger) -> std::io::Result<Self> {
        Self::open_preparing(path, |_, _| {}, log).await
    }

    /// Opens the file; each item written is one JSON line.
    /// An item that cannot be serialised is dropped and counted, and the next one is written.
    pub(crate) async fn open_preparing(
        path: &Path,
        prepare: impl FnMut(&mut T, u64) + Send + 'static,
        log: Logger,
    ) -> std::io::Result<Self> {
        // Read access lets the open inspect the file's end; a file it cannot inspect is an error, not a clean boundary.
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .await
            .inspect_err(|error| {
                log.event(Level::Error, "sink_open_failed", Fields::io(error));
            })?;
        let last = last_byte(&mut file).await.inspect_err(|error| {
            log.event(Level::Error, "sink_open_failed", Fields::io(error));
        })?;
        Ok(Self::writing(file, last, prepare, log))
    }

    /// `last` is the final byte already in the file; a line that follows a non-newline byte starts on its own line.
    fn writing<W: AsyncWrite + Unpin + Send + 'static>(
        file: W,
        last: Option<u8>,
        mut prepare: impl FnMut(&mut T, u64) + Send + 'static,
        log: Logger,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<T>(SINK_QUEUE);
        let dropped = Arc::<AtomicU64>::default();
        let unwritable = dropped.clone();
        tokio::spawn(async move {
            let mut emitted = 0u64;
            let mut file = Tail { inner: file, last };
            while let Some(mut record) = rx.recv().await {
                let Some(sequence) = emitted.checked_add(1) else {
                    log.event(Level::Error, "sequence_exhausted", Fields::default());
                    break;
                };
                prepare(&mut record, sequence);
                let Ok(mut line) = serde_json::to_string(&record) else {
                    unwritable.fetch_add(1, Ordering::Relaxed);
                    log.event(Level::Warn, "sink_encode_failed", Fields::default());
                    continue;
                };
                emitted = sequence;
                line.push('\n');
                if file.last.is_some_and(|byte| byte != b'\n') {
                    line.insert(0, '\n');
                }
                // The writer stays up after an I/O error: a full disk can clear, and a torn line is ended by the next one.
                let failure = match file.write_all(line.as_bytes()).await {
                    Err(error) => Some(("sink_write_failed", error)),
                    Ok(()) => file.flush().await.err().map(|error| ("sink_flush_failed", error)),
                };
                if let Some((event, error)) = failure {
                    unwritable.fetch_add(1, Ordering::Relaxed);
                    log.event(Level::Error, event, Fields::io(&error));
                    continue;
                }
            }
        });
        Self { tx, dropped }
    }
}

async fn last_byte(file: &mut tokio::fs::File) -> std::io::Result<Option<u8>> {
    let len = file.metadata().await?.len();
    if len == 0 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - 1)).await?;
    Ok(Some(file.read_u8().await?))
}

/// Remembers the last byte the inner writer accepted, so a failed write that took part of a line is told apart from one that took none.
struct Tail<W> {
    inner: W,
    last: Option<u8>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for Tail<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if let Poll::Ready(Ok(taken @ 1..)) = polled {
            self.last = Some(bytes[taken - 1]);
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
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
        let queued = self.tx.send(record).await.is_ok();
        if !queued {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queued
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

    struct FailingWriter {
        flush: bool,
    }

    impl tokio::io::AsyncWrite for FailingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(if self.flush {
                Ok(bytes.len())
            } else {
                Err(std::io::ErrorKind::StorageFull.into())
            })
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(if self.flush {
                Err(std::io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            })
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// Takes `partial` bytes of its first write (or fails it outright when 0), fails the next, then stores everything.
    struct FlakyWriter {
        stored: Arc<std::sync::Mutex<Vec<u8>>>,
        calls: u32,
        partial: usize,
    }

    impl tokio::io::AsyncWrite for FlakyWriter {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            self.calls += 1;
            let taken = match (self.calls, self.partial) {
                (1, 0) | (2, 1..) => return std::task::Poll::Ready(Err(std::io::ErrorKind::StorageFull.into())),
                (1, partial) => partial,
                _ => bytes.len(),
            };
            self.stored.lock().unwrap().extend_from_slice(&bytes[..taken]);
            std::task::Poll::Ready(Ok(taken))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_failed_write_is_counted_and_the_lines_after_it_still_get_written_on_their_own_lines() {
        // It ends the torn line, keeps writing, and counts the lost line
        let stored = Arc::<std::sync::Mutex<Vec<u8>>>::default();
        let writer = FlakyWriter {
            stored: stored.clone(),
            calls: 0,
            partial: 1,
        };
        let sink = Sink::writing(writer, None, |_: &mut Value, _| {}, Logger::default());

        for number in [10, 2, 3] {
            sink.write_reliable(json!(number)).await;
        }
        for _ in 0..50 {
            if stored.lock().unwrap().ends_with(b"3\n") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let text = String::from_utf8(stored.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1..], ["2", "3"]);
        assert_eq!(sink.dropped(), 1);
    }

    #[tokio::test]
    async fn a_write_that_took_nothing_does_not_add_a_blank_line() {
        // It keeps the file one record per line when the failed write left no partial line
        let stored = Arc::<std::sync::Mutex<Vec<u8>>>::default();
        let writer = FlakyWriter {
            stored: stored.clone(),
            calls: 0,
            partial: 0,
        };
        let sink = Sink::writing(writer, Some(b'\n'), |_: &mut Value, _| {}, Logger::default());

        for number in [1, 2, 3] {
            sink.write_reliable(json!(number)).await;
        }
        for _ in 0..50 {
            if stored.lock().unwrap().ends_with(b"3\n") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert_eq!(*stored.lock().unwrap(), b"2\n3\n");
        assert_eq!(sink.dropped(), 1);
    }

    #[tokio::test]
    async fn a_file_whose_end_cannot_be_inspected_is_not_opened() {
        // It refuses an append-only file instead of guessing that it ends on a line boundary
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "torn").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();

        let opened = Sink::<Value>::open(&path).await;

        assert!(opened.is_err());
    }

    #[tokio::test]
    async fn opening_a_file_that_ends_mid_line_starts_the_next_line_on_its_own() {
        // It keeps the earlier run's torn line apart from this run's first line
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "torn").unwrap();
        let sink = Sink::open(&path).await.unwrap();

        sink.write_reliable(json!({"first": true})).await;

        wait_for_text(&path, "first").await;
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().collect::<Vec<_>>(), ["torn", r#"{"first":true}"#]);
    }

    #[tokio::test]
    async fn a_reliable_write_the_writer_can_no_longer_take_is_counted() {
        // It reports the refusal and counts the line as dropped
        let (sink, receiver) = Sink::detached(1);
        drop(receiver);

        let written = sink.write_reliable(json!({"late": true})).await;

        assert!(!written);
        assert_eq!(sink.dropped(), 1);
    }

    #[tokio::test]
    async fn disk_write_and_flush_failures_are_reported_through_an_independent_log() {
        use crate::logging::{Format, Logger, Settings};
        use tokio::io::AsyncReadExt;
        for (flush, event, kind) in [
            (false, "sink_write_failed", "storage_full"),
            (true, "sink_flush_failed", "permission_denied"),
        ] {
            let (writer, mut reader) = tokio::io::duplex(8192);
            let log = Logger::new(
                writer,
                Settings {
                    format: Format::Json,
                    ..Settings::default()
                },
            );
            let sink = Sink::writing(FailingWriter { flush }, None, |_: &mut Value, _| {}, log.clone());
            assert!(sink.write_reliable(json!({"body": "synthetic private capture"})).await);
            drop(sink);
            drop(log);
            let mut output = String::new();
            tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_to_string(&mut output))
                .await
                .unwrap()
                .unwrap();
            let records: Vec<Value> = output.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0]["level"], "error");
            assert_eq!(records[0]["event"], event);
            assert_eq!(records[0]["error_kind"], kind);
            assert!(records[0].get("body").is_none());
        }
    }
}
