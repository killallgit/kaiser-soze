use crate::logging::{Fields, Level, Logger};
use serde::Serialize;
use std::ffi::OsStr;
use std::fs::Permissions;
use std::io::SeekFrom;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

/// How large the active file grows before it is closed into a segment, and how much closed output is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    pub segment_bytes: u64,
    pub capacity_bytes: u64,
    pub max_age: std::time::Duration,
}

impl Retention {
    pub const TAP: Self = Self {
        segment_bytes: 64 << 20,
        capacity_bytes: 1024 << 20,
        max_age: std::time::Duration::from_secs(14 * 86_400),
    };
    pub const SHAPES: Self = Self {
        segment_bytes: 4 << 20,
        capacity_bytes: 32 << 20,
        max_age: std::time::Duration::from_secs(14 * 86_400),
    };
}

/// `tap.jsonl` -> `tap.000042.jsonl`; a name without an extension gets `.000042` appended.
pub fn segment_path(active: &Path, index: u64) -> PathBuf {
    let mut name = active.file_stem().unwrap_or_default().to_os_string();
    name.push(format!(".{index:06}"));
    if let Some(extension) = active.extension() {
        name.push(".");
        name.push(extension);
    }
    active.with_file_name(name)
}

/// The index when `name` is `<active stem>.<6 or more ASCII digits>.<active extension>`
/// (or `<active name>.<digits>` without an extension), else None. Compares file names only.
pub fn segment_index(active: &Path, name: &OsStr) -> Option<u64> {
    let stem = active.file_stem()?.to_str()?;
    let rest = name.to_str()?.strip_prefix(stem)?.strip_prefix('.')?;
    let digits = match active.extension() {
        Some(extension) => rest.strip_suffix(extension.to_str()?)?.strip_suffix('.')?,
        None => rest,
    };
    if digits.len() < 6 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

impl<T: Serialize + Send + 'static> Sink<T> {
    #[cfg(test)]
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_reporting(path, Retention::TAP, Logger::default()).await
    }

    pub async fn open_reporting(path: &Path, retention: Retention, log: Logger) -> std::io::Result<Self> {
        Self::open_preparing(path, retention, |_, _| {}, log).await
    }

    /// Opens the file, first closing a non-empty one into a segment; each item written is one JSON line.
    /// An item that cannot be serialised is dropped and counted, and the next one is written.
    pub(crate) async fn open_preparing(
        path: &Path,
        retention: Retention,
        prepare: impl FnMut(&mut T, u64) + Send + 'static,
        log: Logger,
    ) -> std::io::Result<Self> {
        let mut rotation = Rotation::after_existing_segments(path, retention.segment_bytes).await;
        // A symlink is the user's redirect: it is appended to through the link and never renamed.
        let found = tokio::fs::symlink_metadata(path).await;
        let linked = found.as_ref().is_ok_and(|found| found.is_symlink());
        if found.is_ok_and(|found| found.is_file() && found.len() > 0) {
            if let Err(error) = rotation.close_earlier_run().await {
                log.event(Level::Warn, "sink_rotate_failed", Fields::io(&error));
            }
        }
        // Read access lets the open inspect the file's end; a file it cannot inspect is an error, not a clean boundary.
        let mut file = open_active(path).await.inspect_err(|error| {
            log.event(Level::Error, "sink_open_failed", Fields::io(error));
        })?;
        let metadata = file.metadata().await.inspect_err(|error| {
            log.event(Level::Error, "sink_open_failed", Fields::io(error));
        })?;
        let last = secure_and_last_byte(&mut file, &metadata).await.inspect_err(|error| {
            log.event(Level::Error, "sink_open_failed", Fields::io(error));
        })?;
        rotation.limit = metadata.len() + retention.segment_bytes;
        let file = Tail::new(file, last, metadata.len());
        Ok(Self::writing(
            file,
            (metadata.is_file() && !linked).then_some(rotation),
            prepare,
            log,
        ))
    }

    /// A line that follows a non-newline byte starts on its own line. Without `rotation` the file only grows.
    fn writing(
        mut file: Tail,
        mut rotation: Option<Rotation>,
        mut prepare: impl FnMut(&mut T, u64) + Send + 'static,
        log: Logger,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<T>(SINK_QUEUE);
        let dropped = Arc::<AtomicU64>::default();
        let unwritable = dropped.clone();
        tokio::spawn(async move {
            let mut emitted = 0u64;
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
                if let Some(rotation) = rotation
                    .as_mut()
                    .filter(|rotation| rotation.is_due(file.len, line.len()))
                {
                    if let Err(error) = rotation.rotate(&mut file).await {
                        log.event(Level::Warn, "sink_rotate_failed", Fields::io(&error));
                        rotation.limit = file.len + rotation.segment_bytes;
                    }
                }
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

async fn open_active(path: &Path) -> std::io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .await
}

/// Closes the active regular file into numbered segments; `next` is never reused.
struct Rotation {
    active: PathBuf,
    segment_bytes: u64,
    limit: u64,
    next: u64,
}

impl Rotation {
    /// Starts numbering after the highest segment already beside `active`.
    async fn after_existing_segments(active: &Path, segment_bytes: u64) -> Self {
        let mut highest = 0;
        let parent = active.parent().filter(|parent| !parent.as_os_str().is_empty());
        if let Ok(mut entries) = tokio::fs::read_dir(parent.unwrap_or(Path::new("."))).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                highest = highest.max(segment_index(active, &entry.file_name()).unwrap_or(0));
            }
        }
        Self {
            active: active.to_path_buf(),
            segment_bytes,
            limit: segment_bytes,
            next: highest + 1,
        }
    }

    fn is_due(&self, len: u64, line: usize) -> bool {
        len > 0 && len + line as u64 > self.limit
    }

    /// An earlier run's file may predate owner-only permissions, so its segment is tightened once closed.
    async fn close_earlier_run(&mut self) -> std::io::Result<()> {
        let segment = self.close_active().await?;
        tokio::fs::set_permissions(segment, Permissions::from_mode(0o600)).await
    }

    /// Renames the active file to the next segment, never onto an existing path, since rename overwrites.
    async fn close_active(&mut self) -> std::io::Result<PathBuf> {
        let segment = segment_path(&self.active, self.next);
        if tokio::fs::try_exists(&segment).await? {
            return Err(std::io::ErrorKind::AlreadyExists.into());
        }
        tokio::fs::rename(&self.active, &segment).await?;
        self.next += 1;
        Ok(segment)
    }

    /// On an error `file` still appends to the file it had, under the active name when the rename back succeeds.
    async fn rotate(&mut self, file: &mut Tail) -> std::io::Result<()> {
        file.flush().await?;
        let segment = self.close_active().await?;
        match open_active(&self.active).await {
            Ok(opened) => {
                *file = Tail::new(opened, None, 0);
                self.limit = self.segment_bytes;
                Ok(())
            }
            Err(error) => {
                let _ = tokio::fs::rename(&segment, &self.active).await;
                Err(error)
            }
        }
    }
}

/// Makes a regular file owner-only (the open mode applies only to a new file; a device is left as it is), then returns its final byte.
async fn secure_and_last_byte(file: &mut tokio::fs::File, metadata: &std::fs::Metadata) -> std::io::Result<Option<u8>> {
    if metadata.is_file() && metadata.permissions().mode() & 0o077 != 0 {
        file.set_permissions(Permissions::from_mode(0o600)).await?;
    }
    let len = metadata.len();
    if len == 0 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - 1)).await?;
    Ok(Some(file.read_u8().await?))
}

/// Remembers the last byte the inner writer accepted, so a failed write that took part of a line is told apart from one that took none,
/// and the file's length, so the rotation check needs no extra syscall.
struct Tail {
    inner: Box<dyn AsyncWrite + Unpin + Send>,
    last: Option<u8>,
    len: u64,
}

impl Tail {
    fn new(inner: impl AsyncWrite + Unpin + Send + 'static, last: Option<u8>, len: u64) -> Self {
        Self {
            inner: Box::new(inner),
            last,
            len,
        }
    }
}

impl AsyncWrite for Tail {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if let Poll::Ready(Ok(taken @ 1..)) = polled {
            self.last = Some(bytes[taken - 1]);
            self.len += taken as u64;
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
    async fn opening_an_existing_tap_file_makes_it_readable_only_by_its_owner() {
        // It tightens a file that was created earlier with looser permissions
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        Sink::<Value>::open(&path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn a_device_is_opened_as_it_is_instead_of_being_given_a_new_mode() {
        // It discards the capture to /dev/null as before, without trying to change a device's mode
        let opened = Sink::<Value>::open(Path::new("/dev/null")).await;

        assert!(opened.is_ok());
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
        let sink = Sink::writing(
            Tail::new(writer, None, 0),
            None,
            |_: &mut Value, _| {},
            Logger::default(),
        );

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
        let sink = Sink::writing(
            Tail::new(writer, Some(b'\n'), 0),
            None,
            |_: &mut Value, _| {},
            Logger::default(),
        );

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

    fn tiny(segment_bytes: u64) -> Retention {
        Retention {
            segment_bytes,
            ..Retention::TAP
        }
    }

    fn running_as_root(dir: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(dir).unwrap().uid() == 0
    }

    /// The closed segments in index order, then the active file.
    fn files_in_order(active: &Path) -> Vec<PathBuf> {
        let mut segments: Vec<(u64, PathBuf)> = std::fs::read_dir(active.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter_map(|path| Some((segment_index(active, path.file_name()?)?, path)))
            .collect();
        segments.sort();
        let mut files: Vec<PathBuf> = segments.into_iter().map(|(_, path)| path).collect();
        files.push(active.to_path_buf());
        files
    }

    fn lines_in_order(active: &Path) -> Vec<Value> {
        files_in_order(active)
            .iter()
            .flat_map(|path| {
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .lines()
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect()
    }

    async fn wait_for_lines(active: &Path, count: usize) {
        for _ in 0..50 {
            if files_in_order(active)
                .iter()
                .map(|path| std::fs::read_to_string(path).unwrap_or_default().lines().count())
                .sum::<usize>()
                == count
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the lines never reached the files");
    }

    fn json_log() -> (Logger, tokio::io::DuplexStream) {
        use crate::logging::{Format, Settings};
        let (writer, reader) = tokio::io::duplex(8192);
        let settings = Settings {
            format: Format::Json,
            ..Settings::default()
        };
        (Logger::new(writer, settings), reader)
    }

    /// Closes the sink and its log, then returns every record the writer logged.
    async fn logged(sink: Sink<Value>, log: Logger, mut reader: tokio::io::DuplexStream) -> Vec<Value> {
        use tokio::io::AsyncReadExt;
        drop(sink);
        drop(log);
        let mut output = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_to_string(&mut output))
            .await
            .unwrap()
            .unwrap();
        output.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[test]
    fn a_segment_is_named_after_the_active_file_with_a_six_digit_index() {
        // It puts the index before the extension, or after the name when there is none
        let named = segment_path(Path::new("/d/tap.jsonl"), 42);
        let bare = segment_path(Path::new("/d/tap"), 7);

        assert_eq!(named, Path::new("/d/tap.000042.jsonl"));
        assert_eq!(bare, Path::new("/d/tap.000007"));
        assert_eq!(
            segment_index(Path::new("/d/tap.jsonl"), named.file_name().unwrap()),
            Some(42)
        );
        assert_eq!(segment_index(Path::new("/d/tap"), bare.file_name().unwrap()), Some(7));
    }

    #[test]
    fn only_six_or_more_digits_between_the_stem_and_extension_name_a_segment() {
        // It ignores the active file itself, short indexes, and other files
        let active = Path::new("/d/tap.jsonl");

        let indexes: Vec<Option<u64>> = [
            "tap.jsonl",
            "tap.42.jsonl",
            "tap.00004x.jsonl",
            "shapes.000001.jsonl",
            "tap.1234567.jsonl",
        ]
        .iter()
        .map(|name| segment_index(active, std::ffi::OsStr::new(name)))
        .collect();

        assert_eq!(indexes, [None, None, None, None, Some(1_234_567)]);
    }

    #[tokio::test]
    async fn a_small_segment_size_closes_numbered_segments_at_line_boundaries() {
        // It writes whole lines only, numbers segments from 1, and gives each file mode 0600
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        let sink = Sink::open_reporting(&path, tiny(40), Logger::default()).await.unwrap();

        for number in 0..10 {
            sink.write_reliable(json!({"number": number, "pad": "xxxxxxxx"})).await;
        }
        wait_for_lines(&path, 10).await;

        let files = files_in_order(&path);
        assert!(files.len() > 2);
        assert_eq!(files[0], dir.path().join("tap.000001.jsonl"));
        assert_eq!(files[1], dir.path().join("tap.000002.jsonl"));
        for file in &files {
            assert!(std::fs::read_to_string(file).unwrap().ends_with('\n'));
            assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let numbers: Vec<Value> = lines_in_order(&path)
            .iter()
            .map(|line| line["number"].clone())
            .collect();
        assert_eq!(numbers, (0..10).map(|number| json!(number)).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn a_line_longer_than_a_segment_sits_alone_in_its_segment() {
        // It never splits a line, even one bigger than the segment size
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        let sink = Sink::open_reporting(&path, tiny(16), Logger::default()).await.unwrap();

        sink.write_reliable(json!(1)).await;
        sink.write_reliable(json!("a line much longer than sixteen bytes"))
            .await;
        sink.write_reliable(json!(3)).await;
        wait_for_lines(&path, 3).await;

        let counts: Vec<usize> = files_in_order(&path)
            .iter()
            .map(|file| std::fs::read_to_string(file).unwrap().lines().count())
            .collect();
        assert_eq!(counts, [1, 1, 1]);
    }

    #[tokio::test]
    async fn the_sequence_keeps_counting_across_rotations() {
        // It numbers lines 1..n with no gap when read across segments in index order
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        let sink = Sink::open_preparing(
            &path,
            tiny(30),
            |line: &mut Value, sequence| line["sequence"] = json!(sequence),
            Logger::default(),
        )
        .await
        .unwrap();

        for _ in 0..12 {
            sink.write_reliable(json!({})).await;
        }
        wait_for_lines(&path, 12).await;

        let sequences: Vec<Value> = lines_in_order(&path)
            .iter()
            .map(|line| line["sequence"].clone())
            .collect();
        assert!(files_in_order(&path).len() > 2);
        assert_eq!(sequences, (1..=12).map(|sequence| json!(sequence)).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn opening_a_non_empty_file_closes_it_as_the_segment_after_the_highest() {
        // It rotates the earlier run's file unchanged into the next free index
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(dir.path().join("tap.000007.jsonl"), "{}\n").unwrap();
        std::fs::write(&path, "{\"earlier\":1}\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        Sink::<Value>::open(&path).await.unwrap();

        let segment = dir.path().join("tap.000008.jsonl");
        assert_eq!(std::fs::metadata(&segment).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read(dir.path().join("tap.000008.jsonl")).unwrap(),
            b"{\"earlier\":1}\n"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn opening_an_empty_file_does_not_rotate_it() {
        // It creates no segment for a file with nothing in it
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "").unwrap();

        Sink::<Value>::open(&path).await.unwrap();

        assert_eq!(files_in_order(&path), [path]);
    }

    #[tokio::test]
    async fn a_symlinked_active_file_is_appended_to_through_the_link_and_never_renamed() {
        // It keeps the link at the active path and writes every line to its target, however small the segment size
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.jsonl");
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&target, "{\"earlier\":1}\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let sink = Sink::open_reporting(&path, tiny(20), Logger::default()).await.unwrap();

        for number in 0..3 {
            sink.write_reliable(json!({"number": number, "pad": "xxxxxx"})).await;
        }
        wait_for_lines(&target, 4).await;

        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        assert_eq!(files_in_order(&path), std::slice::from_ref(&path));
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn a_symlink_to_a_device_is_never_renamed() {
        // It writes through a link to /dev/null without renaming the link
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        let sink = Sink::open_reporting(&path, tiny(1), Logger::default()).await.unwrap();

        for number in 0..3 {
            assert!(sink.write_reliable(json!(number)).await);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        assert_eq!(files_in_order(&path), std::slice::from_ref(&path));
        assert_eq!(sink.dropped(), 0);
    }

    #[tokio::test]
    async fn a_device_is_never_renamed() {
        // It keeps writing to /dev/null however small the segment size, without trying a rename
        let (log, reader) = json_log();
        let sink = Sink::open_reporting(Path::new("/dev/null"), tiny(1), log.clone())
            .await
            .unwrap();

        for number in 0..3 {
            assert!(sink.write_reliable(json!(number)).await);
        }
        let dropped = sink.dropped();
        let records = logged(sink, log, reader).await;

        assert!(std::os::unix::fs::FileTypeExt::is_char_device(
            &std::fs::metadata("/dev/null").unwrap().file_type()
        ));
        assert_eq!(dropped, 0);
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn a_rotation_the_directory_refuses_keeps_appending_and_is_reported() {
        // It loses no line and reports one sink_rotate_failed with the error kind only
        let dir = tempfile::tempdir().unwrap();
        if running_as_root(dir.path()) {
            return;
        }
        let path = dir.path().join("tap.jsonl");
        let (log, reader) = json_log();
        let sink = Sink::open_reporting(&path, tiny(60), log.clone()).await.unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

        for number in 0..4 {
            sink.write_reliable(json!({"number": number, "pad": "xxxxxx"})).await;
        }
        wait_for_lines(&path, 4).await;
        let records = logged(sink, log, reader).await;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(files_in_order(&path), std::slice::from_ref(&path));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 4);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["level"], "warn");
        assert_eq!(records[0]["event"], "sink_rotate_failed");
        assert_eq!(records[0]["error_kind"], "permission_denied");
    }

    #[tokio::test]
    async fn an_append_only_file_is_rotated_without_being_read() {
        // It renames a file it cannot read into a segment instead of refusing to start
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "torn").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();

        let opened = Sink::<Value>::open(&path).await;

        assert!(opened.is_ok());
        assert_eq!(std::fs::metadata(dir.path().join("tap.000001.jsonl")).unwrap().len(), 4);
    }

    #[tokio::test]
    async fn a_file_whose_end_cannot_be_inspected_is_not_opened_when_it_cannot_be_rotated() {
        // It refuses an append-only file it could not rename instead of guessing that it ends on a line boundary
        let dir = tempfile::tempdir().unwrap();
        if running_as_root(dir.path()) {
            return;
        }
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "torn").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

        let opened = Sink::<Value>::open(&path).await;

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(opened.is_err());
    }

    #[tokio::test]
    async fn opening_a_file_that_ends_mid_line_closes_it_unchanged_and_starts_a_new_one() {
        // It keeps the earlier run's torn line in its own segment, apart from this run's first line
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tap.jsonl");
        std::fs::write(&path, "torn").unwrap();
        let sink = Sink::open(&path).await.unwrap();

        sink.write_reliable(json!({"first": true})).await;

        wait_for_text(&path, "first").await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tap.000001.jsonl")).unwrap(),
            "torn"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"first\":true}\n");
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
            let sink = Sink::writing(
                Tail::new(FailingWriter { flush }, None, 0),
                None,
                |_: &mut Value, _| {},
                log.clone(),
            );
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
