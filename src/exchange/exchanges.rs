use super::project_task::ProjectTask;
use super::recorder::Recorder;
use super::tap::Recording;
use super::{Clock, ExchangeTap, TapOutput};
use crate::adapter::{Adapter, ExchangeParser, RequestHead};
use crate::graph::Tracker;
use crate::line::Entry;
use crate::note::{ExchangeStart, Label, Note};
use crate::project::ProjectLookups;
use crate::shape::{ShapeLog, ShapedParser};
use std::path::PathBuf;
use std::sync::Arc;

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
    clock: Clock,
    output: TapOutput,
    shapes: Option<Arc<ShapeLog>>,
    threads: Tracker,
    projects: ProjectLookups,
}

impl Exchanges {
    pub fn new(output: TapOutput, shapes: Option<Arc<ShapeLog>>, projects: ProjectLookups) -> Self {
        Self {
            clock: Clock::start(),
            output,
            shapes,
            threads: Tracker::default(),
            projects,
        }
    }

    /// Starts tapping one exchange: writes `exchange_start` now, and the edges once the project is found.
    pub fn begin(&self, opening: Opening<'_>) -> ExchangeTap {
        let start = exchange_start(&opening);
        let identity = opening.adapter.identity(opening.head);
        let steps = self.threads.admit(opening.adapter.name(), &identity);
        let exchange = steps.exchange_id();
        let recorder = Recorder::start(self.output.clone(), exchange, opening.adapter.name(), identity.clone());
        let parser: Box<dyn ExchangeParser> = match &self.shapes {
            Some(log) => Box::new(ShapedParser::new(log.clone(), opening.parser, exchange, opening.head)),
            None => opening.parser,
        };
        recorder.write(self.clock.stamp(), Entry::Note(start), (0, 0), None);
        let (project, start_edges) = ProjectTask {
            recorder: recorder.clone(),
            clock: self.clock,
            identity,
            dir: opening.label.map(|label| PathBuf::from(&label.dir)),
            projects: self.projects.clone(),
        }
        .spawn();
        let recording = Recording {
            recorder,
            steps,
            project,
        };
        ExchangeTap::spawn(recording, self.clock, parser, start_edges)
    }

    /// Writes one line under an exchange id of its own, for something that happened without an exchange.
    pub fn note_alone(&self, adapter: &'static str, note: Note) {
        let recorder = Recorder::new(self.output.clone(), self.threads.note_exchange(), adapter);
        recorder.write(self.clock.stamp(), Entry::Note(note), (0, 0), None);
    }
}

fn exchange_start(opening: &Opening<'_>) -> Note {
    Note::ExchangeStart(ExchangeStart {
        headers: opening.head.headers.clone(),
        host: opening.head.host.clone(),
        http_version: format!("{:?}", opening.http_version),
        label: opening.label.cloned(),
        method: opening.head.method.clone(),
        path: opening.head.path.clone(),
        upgrade: opening.upgrade,
    })
}
