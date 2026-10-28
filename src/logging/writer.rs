use super::formatting::encode;
use super::{timestamp_ms, Counts, Fields, Format, Level, Logger, Msg, Record};
use std::sync::atomic::Ordering;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

#[derive(Default)]
pub(super) struct Destination {
    pub format: Format,
    pub limit: Option<u64>,
    pub diagnostics: Logger,
}

struct Writer<W> {
    io: W,
    destination: Destination,
    bytes: u64,
}

enum Outcome {
    Written,
    Oversized,
    Full,
}

pub(super) async fn run<W: AsyncWrite + Unpin>(
    mut rx: mpsc::Receiver<Msg>,
    io: W,
    counts: Counts,
    destination: Destination,
) {
    let mut writer = Writer {
        io,
        destination,
        bytes: 0,
    };
    let mut reported = 0;
    while let Some(message) = rx.recv().await {
        let record = match message {
            Msg::Record(record) => record,
            Msg::Flush(done) => {
                let result = writer.io.flush().await;
                if let Err(error) = &result {
                    writer
                        .destination
                        .diagnostics
                        .event(Level::Warn, "debug_file_flush_failed", Fields::io(error));
                }
                let _ = done.send(result.is_ok());
                continue;
            }
        };
        let dropped = counts.dropped.load(Ordering::Relaxed);
        if dropped > reported {
            let loss = Record {
                timestamp_ms: timestamp_ms(),
                level: Level::Warn,
                event: "log_dropped",
                component: "logging",
                run_id: None,
                fields: Fields {
                    dropped: Some(dropped),
                    ..Fields::default()
                },
            };
            if !writer.emit(&loss, &counts).await {
                break;
            }
            reported = dropped;
        }
        if !writer.emit(&record, &counts).await {
            break;
        }
    }
}

impl<W: AsyncWrite + Unpin> Writer<W> {
    async fn emit(&mut self, record: &Record, counts: &Counts) -> bool {
        match self.write(record).await {
            Ok(Outcome::Written) => true,
            Ok(Outcome::Oversized) => {
                counts.dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
            Ok(Outcome::Full) => {
                counts.dropped.fetch_add(1, Ordering::Relaxed);
                self.destination.diagnostics.event(
                    Level::Warn,
                    "debug_file_limit",
                    Fields {
                        bytes: Some(self.bytes),
                        ..Fields::default()
                    },
                );
                false
            }
            Err(error) => {
                counts.dropped.fetch_add(1, Ordering::Relaxed);
                self.destination
                    .diagnostics
                    .event(Level::Warn, "debug_file_write_failed", Fields::io(&error));
                false
            }
        }
    }

    async fn write(&mut self, record: &Record) -> std::io::Result<Outcome> {
        let Some(line) = encode(record, self.destination.format) else {
            return Ok(Outcome::Oversized);
        };
        if self
            .destination
            .limit
            .is_some_and(|limit| self.bytes + line.len() as u64 > limit)
        {
            return Ok(Outcome::Full);
        }
        self.io.write_all(line.as_bytes()).await?;
        self.bytes += line.len() as u64;
        self.io.flush().await?;
        Ok(Outcome::Written)
    }
}
