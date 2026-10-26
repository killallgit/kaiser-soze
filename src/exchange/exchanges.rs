use super::recorder::ProjectSignal;
use super::{Clock, ExchangeTap, Recorder};
use crate::adapter::{Adapter, ExchangeParser, RequestHead};
use crate::graph::{self, Identity, Threads};
use crate::label::Label;
use crate::project::ProjectLookups;
use crate::shape::{ShapeLog, ShapedParser};
use crate::sink::Sink;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One request an adapter claimed, as `Exchanges::begin` takes it.
pub struct Opening<'a> {
    pub adapter: &'a dyn Adapter,
    pub head: &'a RequestHead,
    pub parser: Box<dyn ExchangeParser>,
    pub label: Option<&'a Label>,
    pub http_version: hyper::Version,
    pub upgrade: bool,
}

/// Gives every exchange its id, its record and its edges. Nothing here waits for the project lookup or the sink.
pub struct Exchanges {
    next_exchange: AtomicU64,
    clock: Clock,
    sink: Sink,
    shapes: Option<Arc<ShapeLog>>,
    threads: Arc<Mutex<Threads>>,
    projects: ProjectLookups,
}

impl Exchanges {
    pub fn new(sink: Sink, shapes: Option<Arc<ShapeLog>>, projects: ProjectLookups) -> Self {
        Self {
            next_exchange: AtomicU64::new(1),
            clock: Clock::start(),
            sink,
            shapes,
            threads: Arc::default(),
            projects,
        }
    }

    /// Starts tapping one exchange: writes `exchange_start` now, and the edges once the project is found.
    pub fn begin(&self, opening: Opening<'_>) -> ExchangeTap {
        let start = start_payload(&opening);
        let exchange = self.next_exchange();
        let identity = opening.adapter.identity(opening.head);
        let recorder = Recorder::new(self.sink.clone(), exchange, opening.adapter.name())
            .knowing(identity.clone())
            .awaiting_project()
            .tracking(self.threads.clone());
        let signal = recorder.project_signal();
        let parser: Box<dyn ExchangeParser> = match &self.shapes {
            Some(log) => Box::new(ShapedParser::new(log.clone(), opening.parser, exchange, opening.head)),
            None => opening.parser,
        };
        let tap = ExchangeTap::spawn(recorder.clone(), self.clock, parser);
        tap.note("exchange_start", with_label(start, opening.label));
        tokio::spawn(
            StartEdges {
                recorder,
                clock: self.clock,
                identity,
                dir: opening.label.map(|label| PathBuf::from(&label.dir)),
                projects: self.projects.clone(),
                signal,
                _open: tap.clone(),
            }
            .write(),
        );
        tap
    }

    /// Writes one line under an exchange id of its own, for something that happened without an exchange.
    pub fn note_alone(&self, adapter: &'static str, kind: &str, payload: Value, label: Option<&Label>) {
        let recorder = Recorder::new(self.sink.clone(), self.next_exchange(), adapter);
        recorder.write(self.clock.stamp(), kind, with_label(payload, label), (0, 0));
    }

    fn next_exchange(&self) -> u64 {
        self.next_exchange.fetch_add(1, Ordering::Relaxed)
    }
}

fn start_payload(opening: &Opening<'_>) -> Value {
    json!({
        "method": opening.head.method,
        "host": opening.head.host,
        "path": opening.head.path,
        "http_version": format!("{:?}", opening.http_version),
        "upgrade": opening.upgrade,
        "headers": opening.head.headers,
    })
}

fn with_label(mut payload: Value, label: Option<&Label>) -> Value {
    if let Some(label) = label {
        payload["label"] = label.to_json();
    }
    payload
}

/// Finds the project of the exchange's directory, settles the record's project signal, then writes the edges.
struct StartEdges {
    recorder: Recorder,
    clock: Clock,
    identity: Identity,
    dir: Option<PathBuf>,
    projects: ProjectLookups,
    signal: ProjectSignal,
    /// Keeps the parser loop, and so `exchange_end`, from finishing before the edges are written.
    _open: ExchangeTap,
}

impl StartEdges {
    async fn write(self) {
        let project = match self.dir {
            Some(dir) => self.projects.find(dir).await,
            None => None,
        };
        self.signal.settle(project.as_ref().map(|project| project.id.clone()));
        for edge in graph::edges(self.recorder.exchange, &self.identity, project.as_ref()) {
            self.recorder.write_edge(self.clock.stamp(), &edge).await;
        }
    }
}
