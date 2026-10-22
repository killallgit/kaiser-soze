use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

pub(super) const UNSET: u64 = u64::MAX;

pub struct Stats {
    pub(super) began: Instant,
    pub(super) chunks: AtomicU64,
    pub(super) bytes: AtomicU64,
    pub(super) dropped: AtomicU64,
    pub(super) head_us: AtomicU64,
    pub(super) first_byte_us: AtomicU64,
    pub(super) forward_sum_us: AtomicU64,
    pub(super) forward_max_us: AtomicU64,
    pub(super) ws_desynced: [AtomicBool; 2],
}

impl Stats {
    pub(super) fn new() -> Self {
        Self {
            began: Instant::now(),
            chunks: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            head_us: AtomicU64::new(UNSET),
            first_byte_us: AtomicU64::new(UNSET),
            forward_sum_us: AtomicU64::new(0),
            forward_max_us: AtomicU64::new(0),
            ws_desynced: Default::default(),
        }
    }

    pub fn response_head_arrived(&self) {
        self.head_us
            .fetch_min(self.began.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    pub fn response_chunk_arrived(&self, len: usize) {
        self.first_byte_us
            .fetch_min(self.began.elapsed().as_micros() as u64, Ordering::Relaxed);
        self.chunks.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(len as u64, Ordering::Relaxed);
    }

    pub fn chunk_handed_on(&self, since_received: std::time::Duration) {
        let us = since_received.as_micros() as u64;
        self.forward_sum_us.fetch_add(us, Ordering::Relaxed);
        self.forward_max_us.fetch_max(us, Ordering::Relaxed);
    }

    pub(super) fn summary(&self) -> Value {
        let unset_as_null = |cell: &AtomicU64| match cell.load(Ordering::Relaxed) {
            UNSET => Value::Null,
            us => json!(us),
        };
        json!({
            "chunks": self.chunks.load(Ordering::Relaxed),
            "bytes": self.bytes.load(Ordering::Relaxed),
            "tap_dropped": self.dropped.load(Ordering::Relaxed),
            "response_head_us": unset_as_null(&self.head_us),
            "first_byte_us": unset_as_null(&self.first_byte_us),
            "forward_sum_us": self.forward_sum_us.load(Ordering::Relaxed),
            "forward_max_us": self.forward_max_us.load(Ordering::Relaxed),
        })
    }
}
