use crate::adapter::{ExchangeParser, TapEvent, TapInput};
use crate::sink::Sink;
use crate::ws::WsDecoder;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const TAP_QUEUE: usize = 256;
const UNSET: u64 = u64::MAX;

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
}

impl Recorder {
    pub fn write(&self, stamp: Stamp, kind: &str, payload: Value, offsets: (u64, u64)) {
        self.sink.write(json!({
            "exchange": self.exchange,
            "adapter": self.adapter,
            "event": kind,
            "mono_us": stamp.mono_us,
            "wall_ms": stamp.wall_ms,
            "start": offsets.0,
            "end": offsets.1,
            "payload": payload,
        }));
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

    pub fn note(&self, kind: &str, payload: Value) {
        self.recorder.write(self.clock.stamp(), kind, payload, (0, 0));
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
            let TapEvent {
                kind,
                payload,
                start,
                end,
            } = event;
            recorder.write(stamp, &kind, payload, (start, end));
        }
    }
    let mut summary = stats.summary();
    summary["sink_dropped"] = json!(recorder.sink.dropped());
    recorder.write(clock.stamp(), "exchange_end", summary, (0, 0));
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
mod tests {
    use super::*;
    use crate::adapter::TapInput;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver;

    struct BlocksUntilReleased {
        release: Receiver<()>,
        blocked: bool,
        messages_seen: Arc<AtomicUsize>,
    }

    impl ExchangeParser for BlocksUntilReleased {
        fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
            if !std::mem::replace(&mut self.blocked, true) {
                self.release.recv().ok();
            }
            if matches!(input, TapInput::WsMessage { .. }) {
                self.messages_seen.fetch_add(1, Ordering::Relaxed);
            }
            Vec::new()
        }
    }

    struct Fixture {
        tap: ExchangeTap,
        release: std::sync::mpsc::Sender<()>,
        messages_seen: Arc<AtomicUsize>,
        tap_file: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    async fn blocked_tap() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let tap_file = dir.path().join("tap.jsonl");
        let sink = Sink::open(&tap_file).await.unwrap();
        let recorder = Recorder {
            sink,
            exchange: 1,
            adapter: "test",
        };
        let (release, receiver) = std::sync::mpsc::channel();
        let messages_seen = Arc::new(AtomicUsize::new(0));
        let parser = BlocksUntilReleased {
            release: receiver,
            blocked: false,
            messages_seen: messages_seen.clone(),
        };
        let tap = ExchangeTap::spawn(recorder, Clock::start(), Box::new(parser));
        Fixture {
            tap,
            release,
            messages_seen,
            tap_file,
            _dir: dir,
        }
    }

    async fn wait_for_exchange_end(tap_file: &std::path::Path) {
        for _ in 0..50 {
            let text = std::fs::read_to_string(tap_file).unwrap_or_default();
            if text.contains("exchange_end") {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("exchange never ended");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalled_parser_drops_and_counts_instead_of_blocking_the_sender() {
        // It counts what the full queue refused while the parser is blocked
        let fixture = blocked_tap().await;

        for _ in 0..1000 {
            fixture.tap.send(TapInput::ResponseBody(Bytes::from_static(b"x")));
        }

        assert!(fixture.tap.stats.dropped.load(Ordering::Relaxed) > 0);
        fixture.release.send(()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_websocket_chunk_stops_that_direction_decoding() {
        // It feeds the parser no messages after a chunk was dropped, instead of decoding garbage
        let Fixture {
            tap,
            release,
            messages_seen,
            tap_file,
            _dir,
        } = blocked_tap().await;
        let text_frame = Bytes::from_static(&[0x81, 0x01, b'a']);

        for _ in 0..1000 {
            tap.send_ws_bytes(true, text_frame.clone());
        }
        release.send(()).unwrap();
        drop(tap);
        wait_for_exchange_end(&tap_file).await;

        assert!(messages_seen.load(Ordering::Relaxed) <= 1);
    }

    #[tokio::test]
    async fn exchange_end_reports_records_the_sink_had_to_drop() {
        // It reports a non-zero sink_dropped when the sink queue overflowed
        let dir = tempfile::tempdir().unwrap();
        let tap_file = dir.path().join("tap.jsonl");
        let sink = Sink::open(&tap_file).await.unwrap();
        let recorder = Recorder {
            sink,
            exchange: 1,
            adapter: "test",
        };
        let (_release, receiver) = std::sync::mpsc::channel();
        let parser = BlocksUntilReleased {
            release: receiver,
            blocked: true,
            messages_seen: Arc::default(),
        };
        let tap = ExchangeTap::spawn(recorder.clone(), Clock::start(), Box::new(parser));
        for _ in 0..5000 {
            recorder.write(Clock::start().stamp(), "filler", json!({}), (0, 0));
        }

        drop(tap);
        wait_for_exchange_end(&tap_file).await;

        let text = std::fs::read_to_string(&tap_file).unwrap();
        let end: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert!(end["payload"]["sink_dropped"].as_u64().unwrap() > 0);
    }
}
