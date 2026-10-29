use crate::line::Line;
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
    /// Opens the tap file. The writer assigns each tap line its 1-based sequence after dequeue.
    pub async fn open(path: &Path, logging: Logger) -> io::Result<Self> {
        let run_id = run_id()?;
        let logging = logging.for_run(run_id.clone());
        let identity = run_id.clone();
        let sink = Sink::open_preparing(
            path,
            move |line: &mut Line, sequence| {
                line.run_id = identity.clone();
                line.sequence = sequence;
            },
            logging.component("tap"),
        )
        .await?;
        Ok(Self { sink, run_id, logging })
    }

    #[cfg(test)]
    pub(super) fn detached(capacity: usize) -> io::Result<(Self, tokio::sync::mpsc::Receiver<Line>)> {
        let (sink, receiver) = Sink::detached(capacity);
        let output = Self {
            sink,
            run_id: run_id()?,
            logging: Logger::default(),
        };
        Ok((output, receiver))
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

#[cfg(test)]
mod tests;
