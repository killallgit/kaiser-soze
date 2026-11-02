use super::clock::Stamp;
use super::TapOutput;
use crate::graph::{Edge, Identity, Tracker};
use crate::line::{Entry, Line};
use crate::record::Record;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;

/// `None` while the lookup runs; then `Some` of what it found.
type ProjectLookup = Option<Option<String>>;

#[derive(Clone)]
pub(crate) struct Recorder {
    pub(super) output: TapOutput,
    pub(super) exchange: u64,
    adapter: &'static str,
    refused: Arc<AtomicU64>,
    pub(super) identity: Identity,
    session: Option<Arc<str>>,
    thread: Option<Arc<str>>,
    project: Arc<watch::Sender<ProjectLookup>>,
    pub(super) threads: Option<Tracker>,
}

pub(super) struct Tracking {
    pub identity: Identity,
    pub threads: Option<Tracker>,
}

impl Recorder {
    pub(super) fn new(output: TapOutput, exchange: u64, adapter: &'static str) -> Self {
        Self {
            output,
            exchange,
            adapter,
            refused: Arc::default(),
            identity: Identity::default(),
            session: None,
            thread: None,
            project: Arc::new(watch::channel(Some(None)).0),
            threads: None,
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
        recorder.session = tracking.identity.session.as_deref().map(Arc::from);
        recorder.thread = tracking.identity.thread.as_deref().map(Arc::from);
        recorder.identity = tracking.identity;
        recorder.threads = tracking.threads;
        recorder.project.send_replace(None);
        let signal = ProjectSignal(recorder.clone());
        (recorder, signal)
    }

    pub(super) async fn settled_project(&self) -> Option<String> {
        let mut lookup = self.project.subscribe();
        let found = lookup.wait_for(Option::is_some).await.ok()?.clone();
        found.flatten()
    }

    fn with_ids(&self, mut record: Record, project: Option<String>) -> Record {
        record.project = project;
        record.session = self.identity.session.clone();
        record.thread = self.identity.thread.clone();
        record
    }

    /// Live lines of this exchange the sink refused.
    pub(super) fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    fn line(&self, stamp: Stamp, entry: Entry, offsets: (u64, u64), step: Option<u64>) -> Line {
        Line {
            run_id: self.output.run_id.clone(),
            sequence: 0,
            exchange: self.exchange,
            adapter: self.adapter,
            mono_us: stamp.mono_us,
            wall_ms: stamp.wall_ms,
            start: offsets.0,
            end: offsets.1,
            session: self.session.clone(),
            thread: self.thread.clone(),
            thread_role: self.identity.role,
            step,
            entry,
        }
    }

    /// Writes a line the sink may refuse; a refused line is counted.
    pub(super) fn write(&self, stamp: Stamp, entry: Entry, offsets: (u64, u64), step: Option<u64>) {
        let line = self.line(stamp, entry, offsets, step);
        self.output.logging.tap(&line);
        if !self.output.sink.write(line) {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Writes a line that waits for room in the sink rather than being refused.
    pub(super) async fn write_reliable(&self, stamp: Stamp, entry: Entry) {
        let line = self.line(stamp, entry, (0, 0), None);
        self.output.logging.tap(&line);
        self.output.sink.write_reliable(line).await;
    }

    pub(super) async fn write_edge(&self, stamp: Stamp, edge: Edge) {
        self.write_reliable(stamp, Entry::Edge(edge)).await;
    }

    pub(super) async fn write_record(&self, stamp: Stamp, record: Record, project: Option<String>) {
        let record = self.with_ids(record, project);
        self.write_reliable(stamp, Entry::Record(Box::new(record))).await;
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
