use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
pub struct Stamp {
    pub(super) mono_us: u64,
    pub(super) wall_ms: u64,
}

#[derive(Clone, Copy)]
pub struct Clock {
    origin: Instant,
}

impl Clock {
    pub fn start() -> Self {
        Self { origin: Instant::now() }
    }

    pub fn stamp(&self) -> Stamp {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Stamp {
            mono_us: self.origin.elapsed().as_micros() as u64,
            wall_ms: wall.as_millis() as u64,
        }
    }
}
