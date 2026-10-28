use super::clock::Stamp;
use super::TapOutput;
use crate::graph::{Edge, Identity, Place, Step, Threads};
use crate::line::{Entry, Line};
use crate::record::Record;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::watch;

/// How long a record waits for the earlier exchanges of its thread before it goes ahead regardless.
const TURN_WAIT: Duration = Duration::from_secs(2);

/// `None` while the lookup runs; then `Some` of what it found.
type ProjectLookup = Option<Option<String>>;

#[derive(Clone)]
pub(crate) struct Recorder {
    pub(super) output: TapOutput,
    pub(super) exchange: u64,
    adapter: &'static str,
    refused: Arc<AtomicU64>,
    identity: Identity,
    project: Arc<watch::Sender<ProjectLookup>>,
    threads: Option<Arc<Mutex<Threads>>>,
    place: Option<Arc<Place>>,
}

pub(super) struct Tracking {
    pub identity: Identity,
    pub threads: Option<Arc<Mutex<Threads>>>,
}

impl Recorder {
    pub(super) fn new(output: TapOutput, exchange: u64, adapter: &'static str) -> Self {
        Self {
            output,
            exchange,
            adapter,
            refused: Arc::default(),
            identity: Identity::default(),
            project: Arc::new(watch::channel(Some(None)).0),
            threads: None,
            place: None,
        }
    }

    /// Starts a tracked recorder with the signal that settles or releases its project wait.
    pub(super) fn start(
        output: TapOutput,
        exchange: u64,
        adapter: &'static str,
        tracking: Tracking,
    ) -> (Self, ProjectSignal) {
        let mut recorder = Self::new(output, exchange, adapter);
        recorder.identity = tracking.identity;
        if let (Some(thread), Some(threads)) = (&recorder.identity.thread, &tracking.threads) {
            let joined = threads
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .join(thread, exchange);
            recorder.place = Some(Arc::new(joined));
        }
        recorder.threads = tracking.threads;
        recorder.project.send_replace(None);
        let signal = ProjectSignal(recorder.clone());
        (recorder, signal)
    }

    async fn settled_project(&self) -> Option<String> {
        let mut lookup = self.project.subscribe();
        let found = lookup.wait_for(Option::is_some).await.ok()?.clone();
        found.flatten()
    }

    fn with_ids(&self, mut record: Record, project: Option<String>, step: Option<&Step>) -> Record {
        record.project = project;
        record.session = self.identity.session.clone();
        record.thread = self.identity.thread.clone();
        if let Some(turn) = step.and_then(|step| step.turn.as_ref()) {
            record.turn = Some(turn.clone());
        }
        record
    }

    /// Live lines of this exchange the sink refused.
    pub(super) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    fn line(&self, stamp: Stamp, entry: Entry, offsets: (u64, u64)) -> Line {
        Line {
            run_id: self.output.run_id.clone(),
            sequence: 0,
            exchange: self.exchange,
            adapter: self.adapter,
            mono_us: stamp.mono_us,
            wall_ms: stamp.wall_ms,
            start: offsets.0,
            end: offsets.1,
            entry,
        }
    }

    /// Writes a line the sink may refuse; a refused line is counted.
    pub(super) fn write(&self, stamp: Stamp, entry: Entry, offsets: (u64, u64)) {
        let line = self.line(stamp, entry, offsets);
        self.output.logging.tap(&line);
        if !self.output.sink.write(line) {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Writes a line that waits for room in the sink rather than being refused.
    pub(super) async fn write_reliable(&self, stamp: Stamp, entry: Entry) {
        let line = self.line(stamp, entry, (0, 0));
        self.output.logging.tap(&line);
        self.output.sink.write_reliable(line).await;
    }

    pub(super) async fn write_edge(&self, stamp: Stamp, edge: Edge) {
        self.write_reliable(stamp, Entry::Edge(edge)).await;
    }

    pub(super) async fn write_record(&self, stamp: Stamp, record: Record) {
        let project = self.settled_project().await;
        if let Some(place) = &self.place {
            place.wait_for_turn(TURN_WAIT).await;
        }
        let step = Step::of_record(&record, &self.identity, self.exchange);
        let record = self.with_ids(record, project, step.as_ref());
        let edges = self.step_edges(step.as_ref());
        self.write_reliable(stamp, Entry::Record(Box::new(record))).await;
        for edge in edges {
            self.write_edge(stamp, edge).await;
        }
        if let Some(place) = &self.place {
            place.leave();
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
pub(super) struct ProjectSignal(Recorder);

impl ProjectSignal {
    pub(super) fn settle(self, project: Option<String>) {
        self.0.project.send_replace(Some(project));
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
