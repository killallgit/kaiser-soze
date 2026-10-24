use super::clock::Stamp;
use crate::graph::{Identity, Step, Threads};
use crate::sink::Sink;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::watch;

/// How long a record waits for the project lookup before it is written without one.
pub const PROJECT_WAIT: Duration = Duration::from_millis(250);

/// `None` while the lookup runs; then `Some` of what it found.
type ProjectLookup = Option<Option<String>>;

#[derive(Clone)]
pub struct Recorder {
    pub sink: Sink,
    pub exchange: u64,
    pub adapter: &'static str,
    pub(super) refused: Arc<AtomicU64>,
    pub(super) identity: Identity,
    pub(super) project: Arc<watch::Sender<ProjectLookup>>,
    pub(super) project_wait: Duration,
    threads: Option<Arc<Mutex<Threads>>>,
}

impl Recorder {
    pub fn new(sink: Sink, exchange: u64, adapter: &'static str) -> Self {
        Self {
            sink,
            exchange,
            adapter,
            refused: Arc::default(),
            identity: Identity::default(),
            project: Arc::new(watch::channel(Some(None)).0),
            project_wait: Duration::ZERO,
            threads: None,
        }
    }

    /// Makes this recorder's records carry the session and thread of its exchange.
    pub fn knowing(mut self, identity: Identity) -> Self {
        self.identity = identity;
        self
    }

    /// Makes this recorder's records add their `turn` and `continues` edges to `threads`.
    pub fn tracking(mut self, threads: Arc<Mutex<Threads>>) -> Self {
        self.threads = Some(threads);
        self
    }

    /// Makes this recorder's records wait up to `up_to` for `project_looked_up`.
    pub fn awaiting_project(mut self, up_to: Duration) -> Self {
        self.project.send_replace(None);
        self.project_wait = up_to;
        self
    }

    /// Ends the wait: later records carry the project when one was found.
    pub fn project_looked_up(&self, project: Option<String>) {
        self.project.send_replace(Some(project));
    }

    pub(super) async fn settled_project(&self) -> Option<String> {
        let mut lookup = self.project.subscribe();
        let settled = tokio::time::timeout(self.project_wait, lookup.wait_for(Option::is_some)).await;
        let found = settled.ok()?.ok()?.clone();
        found.flatten()
    }

    pub(super) fn with_ids(&self, mut record: Value, project: Option<String>) -> Value {
        let ids = [
            ("project", project.as_ref()),
            ("session", self.identity.session.as_ref()),
            ("thread", self.identity.thread.as_ref()),
        ];
        for (key, id) in ids {
            if let Some(id) = id {
                record[key] = json!(id);
            }
        }
        // The header turn is the first request's; a later step that names none may be a new turn.
        if record.get("turn").is_none() && record["step"] == 1 {
            if let Some(turn) = &self.identity.turn {
                record["turn"] = json!(turn);
            }
        }
        record
    }

    /// Live lines of this exchange the sink refused.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    pub(super) fn line(&self, stamp: Stamp, kind: &str, payload: Value, offsets: (u64, u64)) -> Value {
        json!({
            "exchange": self.exchange,
            "adapter": self.adapter,
            "event": kind,
            "mono_us": stamp.mono_us,
            "wall_ms": stamp.wall_ms,
            "start": offsets.0,
            "end": offsets.1,
            "payload": payload,
        })
    }

    pub fn write(&self, stamp: Stamp, kind: &str, payload: Value, offsets: (u64, u64)) {
        if !self.sink.write(self.line(stamp, kind, payload, offsets)) {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub async fn write_reliable(&self, stamp: Stamp, kind: &str, payload: Value) {
        self.sink.write_reliable(self.line(stamp, kind, payload, (0, 0))).await;
    }

    pub(super) async fn write_record(&self, stamp: Stamp, record: Value) {
        let project = self.settled_project().await;
        let record = self.with_ids(record, project);
        let edges = self.step_edges(&record);
        self.write_reliable(stamp, "record", record).await;
        for edge in edges {
            self.write_reliable(stamp, "edge", edge).await;
        }
    }

    fn step_edges(&self, record: &Value) -> Vec<Value> {
        let Some(threads) = &self.threads else {
            return Vec::new();
        };
        let text = |value: &Value| value.as_str().map(String::from);
        let Some(n) = record["step"].as_u64() else {
            return Vec::new();
        };
        let step = Step {
            thread: self.identity.thread.clone(),
            exchange: self.exchange,
            n,
            turn: text(&record["turn"]),
            previous_message_id: text(&record["conversation"]["previous_message_id"]),
            message_id: text(&record["reply"]["message_id"]),
        };
        threads.lock().unwrap_or_else(PoisonError::into_inner).edges(&step)
    }
}
