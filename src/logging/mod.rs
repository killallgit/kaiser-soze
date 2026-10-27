mod fields;
mod project;
mod writer;

pub use fields::Fields;

use serde::Serialize;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, oneshot};

const LOG_QUEUE: usize = 256;
const STREAM_PER_SECOND: u64 = 20;
const MAX_COLUMNS: usize = 160;
const MAX_RECORD_BYTES: usize = 1024;
const DEBUG_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

impl FromStr for Level {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            _ => Err("invalid_log_level"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Human,
    Json,
}

impl FromStr for Format {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            _ => Err("invalid_log_format"),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Settings {
    pub level: Level,
    pub format: Format,
    pub live: bool,
    pub stream: bool,
}

#[derive(Default)]
struct Rate(AtomicU64);

impl Rate {
    fn allow_at(&self, second: u64) -> bool {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                if old >> 8 != second {
                    Some((second << 8) | 1)
                } else if old & 255 < STREAM_PER_SECOND {
                    Some(old + 1)
                } else {
                    None
                }
            })
            .is_ok()
    }
}

#[derive(Clone, Default)]
struct Counts {
    dropped: Arc<AtomicU64>,
    suppressed: Arc<AtomicU64>,
}

struct Output {
    tx: mpsc::Sender<Msg>,
    settings: Settings,
    counts: Counts,
    origin: Instant,
    stream_rate: Rate,
    operation_rate: Rate,
    limited: bool,
}

enum Msg {
    Record(Box<Record>),
    Flush(oneshot::Sender<bool>),
}

#[derive(Serialize)]
struct Record {
    timestamp_ms: u64,
    level: Level,
    event: &'static str,
    component: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(flatten)]
    fields: Fields,
}

/// A bounded, nonblocking operational log. It accepts metadata fields, never arbitrary message text.
#[derive(Clone, Default)]
pub struct Logger {
    outputs: Vec<Arc<Output>>,
    component: &'static str,
    run_id: Option<Arc<str>>,
}

impl Logger {
    pub fn new<W: AsyncWrite + Unpin + Send + 'static>(writer: W, settings: Settings) -> Self {
        Self::start(writer, settings, writer::Destination::default())
    }

    fn start<W: AsyncWrite + Unpin + Send + 'static>(
        writer: W,
        settings: Settings,
        mut destination: writer::Destination,
    ) -> Self {
        let (tx, rx) = mpsc::channel(LOG_QUEUE);
        let counts = Counts::default();
        let limited = destination.limit.is_none();
        destination.format = settings.format;
        tokio::spawn(writer::run(rx, writer, counts.clone(), destination));
        Self {
            outputs: vec![Arc::new(Output {
                tx,
                settings,
                counts,
                origin: Instant::now(),
                stream_rate: Rate::default(),
                operation_rate: Rate::default(),
                limited,
            })],
            component: "soze",
            run_id: None,
        }
    }

    fn debug<W: AsyncWrite + Unpin + Send + 'static>(writer: W, diagnostics: Logger, limit: u64) -> Self {
        Self::start(
            writer,
            Settings {
                level: Level::Debug,
                format: Format::Json,
                live: true,
                stream: true,
            },
            writer::Destination {
                format: Format::Json,
                limit: Some(limit),
                diagnostics,
            },
        )
    }

    fn combine(mut self, other: Self) -> Self {
        self.outputs.extend(other.outputs);
        self
    }

    /// Add an independent metadata-only JSONL transport, truncated on open and capped per run.
    pub async fn with_debug_file(self, path: &Path) -> std::io::Result<Self> {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .await?;
        let debug = Self::debug(file, self.clone(), DEBUG_FILE_BYTES);
        Ok(self.combine(debug))
    }

    pub(crate) fn for_run(&self, run_id: Arc<str>) -> Self {
        Self {
            run_id: Some(run_id),
            ..self.clone()
        }
    }

    pub(crate) fn component(&self, component: &'static str) -> Self {
        Self {
            component,
            ..self.clone()
        }
    }

    fn enabled(out: &Output, level: Level) -> bool {
        level != Level::Off && out.settings.level >= level
    }

    pub fn event(&self, level: Level, event: &'static str, fields: Fields) -> bool {
        let mut sent = false;
        for out in &self.outputs {
            if !Self::enabled(out, level) {
                continue;
            }
            if out.limited && level != Level::Error && !out.operation_rate.allow_at(out.origin.elapsed().as_secs()) {
                out.counts.suppressed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            sent |= self.send(out, level, event, fields.clone());
        }
        sent
    }

    fn send(&self, out: &Output, level: Level, event: &'static str, mut fields: Fields) -> bool {
        fields.bound();
        let suppressed = out.counts.suppressed.swap(0, Ordering::Relaxed);
        if suppressed > 0 {
            fields.suppressed = Some(suppressed);
        }
        let record = Record {
            timestamp_ms: timestamp_ms(),
            level,
            event,
            component: self.component,
            run_id: self.run_id.as_ref().map(|id| id.chars().take(64).collect()),
            fields,
        };
        if out.tx.try_send(Msg::Record(Box::new(record))).is_err() {
            out.counts.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    pub fn dropped(&self) -> u64 {
        self.outputs
            .iter()
            .map(|out| out.counts.dropped.load(Ordering::Relaxed))
            .sum()
    }

    /// Bounded drain for startup failure/shutdown only. Never called from forwarding or capture.
    pub async fn flush(&self) -> bool {
        let mut flushed = true;
        for out in &self.outputs {
            flushed &= Self::flush_output(out).await;
        }
        flushed
    }

    async fn flush_output(out: &Output) -> bool {
        tokio::time::timeout(Duration::from_millis(250), async {
            let (tx, rx) = oneshot::channel();
            if out.tx.send(Msg::Flush(tx)).await.is_err() {
                return false;
            }
            rx.await.unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests;
