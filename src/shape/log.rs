use crate::sink::Sink;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

const MAX_SHAPES: usize = 10_000;

/// Writes each distinct shape once per run to its own file.
pub struct ShapeLog {
    sink: Sink<Value>,
    seen: Mutex<HashSet<String>>,
}

impl ShapeLog {
    pub fn new(sink: Sink<Value>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            seen: Mutex::default(),
        })
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
