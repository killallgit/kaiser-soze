use super::clock::Stamp;
use crate::event::Event;
use crate::graph::{Edge, Identity, Step, Threads};
use crate::line::{Entry, Line};
use crate::sink::Sink;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

/// `None` while the lookup runs; then `Some` of what it found.
type ProjectLookup = Option<Option<String>>;

#[derive(Clone)]
pub struct Recorder {
    pub sink: Sink<Line>,
    pub exchange: u64,
    pub adapter: &'static str,
    pub(super) refused: Arc<AtomicU64>,
    pub(super) identity: Identity,
    project: Arc<watch::Sender<ProjectLookup>>,
    threads: Option<Arc<Mutex<Threads>>>,
}

impl Recorder {
    pub fn new(sink: Sink<Line>, exchange: u64, adapter: &'static str) -> Self {
        Self {
            sink,
            exchange,
            adapter,
            refused: Arc::default(),
            identity: Identity::default(),
            project: Arc::new(watch::channel(Some(None)).0),
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

    /// Makes this recorder's records wait for the project signal, which ends when it is settled or dropped.
    /// Take `project_signal` too: a waiting recorder whose signal is never taken writes no record.
    pub fn awaiting_project(self) -> Self {
        self.project.send_replace(None);
        self
    }

    pub fn project_signal(&self) -> ProjectSignal {
        ProjectSignal(self.clone())
    }

    pub(super) fn project_looked_up(&self, project: Option<String>) {
        self.project.send_replace(Some(project));
    }

    pub(super) async fn settled_project(&self) -> Option<String> {
        let mut lookup = self.project.subscribe();
        let found = lookup.wait_for(Option::is_some).await.ok()?.clone();
        found.flatten()
    }

    pub(super) fn with_ids(&self, mut record: Value, project: Option<String>, step: Option<&Step>) -> Value {
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
        if let Some(turn) = step.and_then(|step| step.turn.as_ref()) {
            record["turn"] = json!(turn);
        }
        record
    }

    /// Live lines of this exchange the sink refused.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    fn line(&self, stamp: Stamp, entry: Entry, offsets: (u64, u64)) -> Line {
        Line {
            exchange: self.exchange,
            adapter: self.adapter,
            mono_us: stamp.mono_us,
            wall_ms: stamp.wall_ms,
            start: offsets.0,
            end: offsets.1,
            entry,
        }
    }

    pub fn write(&self, stamp: Stamp, kind: &str, payload: Value, offsets: (u64, u64)) {
        let line = self.line(stamp, Entry::untyped(kind, payload), offsets);
        if !self.sink.write(line) {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Writes a stream event under its own name; an event that cannot be written counts as refused.
    pub fn write_event(&self, stamp: Stamp, event: &Event, offsets: (u64, u64)) {
        match serde_json::to_value(event) {
            Ok(payload) => self.write(stamp, event.kind(), payload, offsets),
            Err(_) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub async fn write_reliable(&self, stamp: Stamp, kind: &str, payload: Value) {
        self.write_entry_reliable(stamp, Entry::untyped(kind, payload)).await;
    }

    pub(super) async fn write_edge(&self, stamp: Stamp, edge: Edge) {
        self.write_entry_reliable(stamp, Entry::Edge(edge)).await;
    }

    async fn write_entry_reliable(&self, stamp: Stamp, entry: Entry) {
        self.sink.write_reliable(self.line(stamp, entry, (0, 0))).await;
    }

    pub(super) async fn write_record(&self, stamp: Stamp, record: Value) {
        let project = self.settled_project().await;
        let step = Step::of_record(&record, &self.identity, self.exchange);
        let record = self.with_ids(record, project, step.as_ref());
        let edges = self.step_edges(step.as_ref());
        self.write_reliable(stamp, "record", record).await;
        for edge in edges {
            self.write_edge(stamp, edge).await;
        }
    }

    fn step_edges(&self, step: Option<&Step>) -> Vec<Edge> {
        let (Some(threads), Some(step)) = (&self.threads, step) else {
            return Vec::new();
        };
        threads.lock().unwrap_or_else(PoisonError::into_inner).edges(step)
    }
}

/// Ends a recorder's wait for its project: later records carry the project when `settle` gave one.
/// Dropped unanswered, it settles with no project, so a record never waits for a lookup that cannot come.
pub struct ProjectSignal(Recorder);

impl ProjectSignal {
    pub fn settle(self, project: Option<String>) {
        self.0.project_looked_up(project);
    }
}

impl Drop for ProjectSignal {
    fn drop(&mut self) {
        self.0.project.send_if_modified(|lookup| {
            let unanswered = lookup.is_none();
            if unanswered {
                *lookup = Some(None);
            }
            unanswered
        });
    }
}
