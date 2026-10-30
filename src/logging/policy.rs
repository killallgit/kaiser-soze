use super::project::{self, Kind};
use super::{Fields, Level, Logger, Output};
use crate::line::Line;
use std::sync::atomic::Ordering;

impl Logger {
    pub fn event(&self, level: Level, event: &'static str, fields: Fields) -> bool {
        let mut sent = false;
        for out in &self.outputs {
            if !out.enabled(level) {
                continue;
            }
            if out.limited && level != Level::Error && !out.operation_rate.allow_at(out.origin.elapsed().as_secs()) {
                out.counts.suppressed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            sent |= self.send(out, level, event, fields.clone());
        }
        sent
    }

    /// Emits selected numeric metadata before the tap writer, independently of disk latency.
    pub fn tap(&self, line: &Line) -> bool {
        let kind = project::kind(&line.entry);
        let mut sent = false;
        for out in &self.outputs {
            if let Some(level) = out.tap_level(kind) {
                sent |= self
                    .for_run(line.run_id.clone())
                    .send(out, level, line.entry.event(), project::fields(line));
            }
        }
        sent
    }
}

impl Output {
    fn enabled(&self, level: Level) -> bool {
        level != Level::Off && self.settings.borrow().level >= level
    }

    fn tap_level(&self, kind: Kind) -> Option<Level> {
        let settings = *self.settings.borrow();
        let operation = matches!(kind, Kind::Warning) || matches!(kind, Kind::Summary) && settings.live;
        let level = match kind {
            Kind::Warning => Level::Warn,
            Kind::Summary if settings.live => Level::Info,
            _ if settings.stream => Level::Debug,
            _ => return None,
        };
        if !self.enabled(level) {
            return None;
        }
        let rate = if operation {
            &self.operation_rate
        } else {
            &self.stream_rate
        };
        if self.limited && !rate.allow_at(self.origin.elapsed().as_secs()) {
            self.counts.suppressed.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(level)
    }
}
