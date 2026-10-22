mod printer;
mod state;
#[cfg(test)]
mod tests;

pub use printer::Printer;
pub use state::LiveState;

use serde_json::Value;
use tokio::io::AsyncWrite;

/// What the sink needs to show lines: the formatter and the printer.
pub struct Live {
    pub state: LiveState,
    pub printer: Printer,
}

impl Live {
    pub fn new<W: AsyncWrite + Send + Unpin + 'static>(writer: W, home: String, offset_secs: i64) -> Self {
        Self {
            state: LiveState::new(home, offset_secs),
            printer: Printer::spawn(writer),
        }
    }

    pub fn show(&mut self, tap: &Value) {
        if let Some(line) = self.state.line(tap) {
            self.printer.offer(line);
        }
    }
}
