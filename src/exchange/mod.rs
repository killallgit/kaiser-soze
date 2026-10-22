use crate::adapter::{ExchangeParser, TapEvent, TapInput};
use crate::assembler::Assembler;
use crate::graph::Identity;
use crate::sink::Sink;
use crate::ws::WsDecoder;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};

const TAP_QUEUE: usize = 256;
const UNSET: u64 = u64::MAX;

/// How long a record waits for the project lookup before it is written without one.
pub const PROJECT_WAIT: Duration = Duration::from_millis(250);

/// `None` while the lookup runs; then `Some` of what it found.
type ProjectLookup = Option<Option<String>>;

#[derive(Clone, Copy)]
pub struct Stamp {
    mono_us: u64,
    wall_ms: u64,
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

pub struct Stats {
    began: Instant,
    chunks: AtomicU64,
    bytes: AtomicU64,
    dropped: AtomicU64,
    head_us: AtomicU64,
    first_byte_us: AtomicU64,
    forward_sum_us: AtomicU64,
    forward_max_us: AtomicU64,
    ws_desynced: [AtomicBool; 2],
}

impl Stats {
    fn new() -> Self {
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

    fn summary(&self) -> Value {
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

#[derive(Clone)]
pub struct Recorder {
    pub sink: Sink,
    pub exchange: u64,
    pub adapter: &'static str,
    refused: Arc<AtomicU64>,
    identity: Identity,
    project: Arc<watch::Sender<ProjectLookup>>,
    project_wait: Duration,
}

impl Recorder {
    pub fn new(sink: Sink, exchange: u64, adapter: &'static str) -> Self {
        Self {
            sink,
            exchange,
            adapter,
            refused: Arc::default(),
            identity: Identity::default(),
            project: Arc::new(watch::channel(Some(None)).0),
            project_wait: Duration::ZERO,
        }
    }

    /// Makes this recorder's records carry the session and thread of its exchange.
    pub fn knowing(mut self, identity: Identity) -> Self {
        self.identity = identity;
        self
    }

    /// Makes this recorder's records wait up to `up_to` for `project_looked_up`.
    pub fn awaiting_project(mut self, up_to: Duration) -> Self {
        self.project.send_replace(None);
        self.project_wait = up_to;
        self
    }

    /// Ends the wait: later records carry the project when one was found.
    pub fn project_looked_up(&self, project: Option<String>) {
        self.project.send_replace(Some(project));
    }

    async fn settled_project(&self) -> Option<String> {
        let mut lookup = self.project.subscribe();
        let settled = tokio::time::timeout(self.project_wait, lookup.wait_for(Option::is_some)).await;
        let found = settled.ok()?.ok()?.clone();
        found.flatten()
    }

    fn with_ids(&self, mut record: Value, project: Option<String>) -> Value {
        let ids = [
            ("project", project.as_ref()),
            ("session", self.identity.session.as_ref()),
            ("thread", self.identity.thread.as_ref()),
        ];
        for (key, id) in ids {
            if let Some(id) = id {
                record[key] = json!(id);
            }
        }
        record
    }

    /// Live lines of this exchange the sink refused.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    fn line(&self, stamp: Stamp, kind: &str, payload: Value, offsets: (u64, u64)) -> Value {
        json!({
            "exchange": self.exchange,
            "adapter": self.adapter,
            "event": kind,
            "mono_us": stamp.mono_us,
            "wall_ms": stamp.wall_ms,
            "start": offsets.0,
            "end": offsets.1,
            "payload": payload,
        })
    }

    pub fn write(&self, stamp: Stamp, kind: &str, payload: Value, offsets: (u64, u64)) {
        if !self.sink.write(self.line(stamp, kind, payload, offsets)) {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub async fn write_reliable(&self, stamp: Stamp, kind: &str, payload: Value) {
        self.sink.write_reliable(self.line(stamp, kind, payload, (0, 0))).await;
    }

    async fn write_record(&self, stamp: Stamp, record: Value) {
        let project = self.settled_project().await;
        self.write_reliable(stamp, "record", self.with_ids(record, project))
            .await;
    }
}

enum Msg {
    Input(TapInput),
    WsBytes { from_client: bool, bytes: Bytes },
}

#[derive(Clone)]
pub struct ExchangeTap {
    tx: mpsc::Sender<(Stamp, Msg)>,
    clock: Clock,
    recorder: Recorder,
    pub stats: Arc<Stats>,
}

impl ExchangeTap {
    pub fn spawn(recorder: Recorder, clock: Clock, parser: Box<dyn ExchangeParser>) -> Self {
        let (tx, rx) = mpsc::channel(TAP_QUEUE);
        let stats = Arc::new(Stats::new());
        tokio::spawn(run_parser(rx, parser, recorder.clone(), clock, stats.clone()));
        Self {
            tx,
            clock,
            recorder,
            stats,
        }
    }

    pub fn project_looked_up(&self, project: Option<String>) {
        self.recorder.project_looked_up(project);
    }

    pub fn note(&self, kind: &str, payload: Value) {
        self.recorder.write(self.clock.stamp(), kind, payload, (0, 0));
    }

    /// Like `note`, but waits for room in the output queue. Only a tap-side task calls this.
    pub async fn note_reliable(&self, kind: &str, payload: Value) {
        self.recorder.write_reliable(self.clock.stamp(), kind, payload).await;
    }

    pub fn send(&self, input: TapInput) {
        self.offer(Msg::Input(input));
    }

    pub fn send_ws_bytes(&self, from_client: bool, bytes: Bytes) {
        self.offer(Msg::WsBytes { from_client, bytes });
    }

    fn offer(&self, msg: Msg) {
        if let Err(refused) = self.tx.try_send((self.clock.stamp(), msg)) {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            if let (_, Msg::WsBytes { from_client, .. }) = refused.into_inner() {
                self.stats.ws_desynced[from_client as usize].store(true, Ordering::Relaxed);
            }
        }
    }
}

async fn run_parser(
    mut rx: mpsc::Receiver<(Stamp, Msg)>,
    mut parser: Box<dyn ExchangeParser>,
    recorder: Recorder,
    clock: Clock,
    stats: Arc<Stats>,
) {
    let mut decoders = [WsDecoder::default(), WsDecoder::default()];
    let mut assembler = Assembler::default();
    let dropped = || stats.dropped.load(Ordering::Relaxed) + recorder.refused();
    while let Some((stamp, msg)) = rx.recv().await {
        let inputs = match msg {
            Msg::Input(input) => vec![input],
            Msg::WsBytes { from_client, bytes } => {
                let decoder = &mut decoders[from_client as usize];
                if stats.ws_desynced[from_client as usize].load(Ordering::Relaxed) {
                    decoder.mark_broken();
                }
                decoder
                    .push(&bytes)
                    .into_iter()
                    .map(|m| TapInput::WsMessage {
                        from_client,
                        text: m.text,
                        start: m.start,
                        end: m.end,
                    })
                    .collect()
            }
        };
        for event in inputs.into_iter().flat_map(|input| parser.feed(input)) {
            let records = assembler.push(&event, dropped());
            let TapEvent {
                kind,
                payload,
                start,
                end,
            } = event;
            recorder.write(stamp, &kind, payload, (start, end));
            for record in records {
                recorder.write_record(stamp, record).await;
            }
        }
    }
    if let Some(record) = assembler.finish(dropped()) {
        recorder.write_record(clock.stamp(), record).await;
    }
    let mut summary = stats.summary();
    summary["sink_dropped"] = json!(recorder.sink.dropped());
    recorder.write_reliable(clock.stamp(), "exchange_end", summary).await;
}

#[derive(Clone, Copy, PartialEq)]
pub enum Side {
    Request,
    Response,
}

pub struct TeeBody<B> {
    inner: B,
    tap: Option<ExchangeTap>,
    side: Side,
    received_at: Option<Instant>,
    ended: bool,
}

impl<B> TeeBody<B> {
    pub fn new(inner: B, tap: Option<ExchangeTap>, side: Side) -> Self {
        Self {
            inner,
            tap,
            side,
            received_at: None,
            ended: false,
        }
    }
}

impl<B: Body<Data = Bytes> + Unpin> Body for TeeBody<B> {
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        let Some(tap) = &this.tap else {
            return polled;
        };
        if let Some(received_at) = this.received_at.take() {
            tap.stats.chunk_handed_on(received_at.elapsed());
        }
        if let Poll::Ready(Some(Ok(frame))) = &polled {
            if let Some(data) = frame.data_ref() {
                this.received_at = Some(Instant::now());
                tap.send(match this.side {
                    Side::Request => TapInput::RequestBody(data.clone()),
                    Side::Response => TapInput::ResponseBody(data.clone()),
                });
                if this.side == Side::Response {
                    tap.stats.response_chunk_arrived(data.len());
                }
            }
        }
        // hyper stops polling once a body reports its end, so the end is noted here too
        let finished = matches!(polled, Poll::Ready(None)) || this.inner.is_end_stream();
        if finished && !this.ended {
            this.ended = true;
            tap.send(match this.side {
                Side::Request => TapInput::RequestEnd,
                Side::Response => TapInput::ResponseEnd,
            });
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests;
