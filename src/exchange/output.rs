use crate::line::Line;
use crate::live::Live;
use crate::logging::Logger;
use crate::sink::Sink;
use rustls::crypto::ring;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// One tap-writer epoch, shared by every exchange and recorder that writes to it.
#[derive(Clone)]
pub struct TapOutput {
    pub(super) sink: Sink<Line>,
    pub(super) run_id: Arc<str>,
    pub(crate) logging: Logger,
}

impl TapOutput {
    /// Generates the opaque capture identity once, before any tap line is emitted.
    pub fn new(sink: Sink<Line>) -> io::Result<Self> {
        Ok(Self {
            sink,
            run_id: run_id()?,
            logging: Logger::default(),
        })
    }

    pub async fn open(path: &Path, live: Option<Live>, logging: Logger) -> io::Result<Self> {
        let run_id = run_id()?;
        let logging = logging.for_run(run_id.clone());
        let sink = Sink::open_with_reporting(path, live, logging.component("tap")).await?;
        Ok(Self { sink, run_id, logging })
    }
}

fn run_id() -> io::Result<Arc<str>> {
    let mut random = [0u8; 16];
    ring::default_provider()
        .secure_random
        .fill(&mut random)
        .map_err(|_| io::Error::other("capture run identity generation failed"))?;
    let run_id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(run_id.into())
}
