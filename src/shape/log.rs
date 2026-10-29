use crate::logging::{Fields, Level, Logger};
use crate::sink::Sink;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const MAX_SHAPES: usize = 10_000;

/// Writes each distinct shape once per run to its own file.
pub struct ShapeLog {
    sink: Sink<Value>,
    seen: Mutex<HashSet<String>>,
    response_dropped: AtomicU64,
    logging: Logger,
}

impl ShapeLog {
    pub fn new(sink: Sink<Value>) -> Arc<Self> {
        Self::with_logging(sink, Logger::default())
    }

    pub(crate) fn with_logging(sink: Sink<Value>, logging: Logger) -> Arc<Self> {
        Arc::new(Self {
            sink,
            seen: Mutex::default(),
            response_dropped: AtomicU64::new(0),
            logging,
        })
    }

    pub(super) fn drop_response(&self, exchange: u64) {
        let dropped = self.response_dropped.fetch_add(1, Ordering::Relaxed) + 1;
        self.logging.event(
            Level::Warn,
            "shape_response_limit_exceeded",
            Fields {
                exchange: Some(exchange),
                dropped: Some(dropped),
                ..Fields::default()
            },
        );
    }

    pub(super) fn record(&self, exchange: u64, line: Value) {
        let key = line.to_string();
        let Ok(mut seen) = self.seen.lock() else { return };
        if seen.len() >= MAX_SHAPES || !seen.insert(key) {
            return;
        }
        drop(seen);
        let mut line = line;
        line["exchange"] = json!(exchange);
        self.sink.write(line);
    }
}
