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
mod tests {
    use super::*;
    use crate::adapter::TapInput;
    use crate::graph::Identity;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

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
        let recorder = Recorder::new(sink, 1, "test");
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
        let recorder = Recorder::new(sink, 1, "test");
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

    struct Emits {
        events: Vec<TapEvent>,
        release: Option<Receiver<()>>,
    }

    impl ExchangeParser for Emits {
        fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
            if let Some(release) = self.release.take() {
                release.recv().ok();
            }
            if matches!(input, TapInput::ResponseEnd) {
                std::mem::take(&mut self.events)
            } else {
                Vec::new()
            }
        }
    }

    fn open_reply() -> Vec<TapEvent> {
        use crate::common::*;
        vec![
            conversation(json!("sys"), json!([]), vec![message("user", vec![text_block("hi")])]),
            block_start(BlockStart {
                index: 0,
                kind: "text",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Text("ok".into())),
        ]
    }

    fn closed_reply() -> Vec<TapEvent> {
        use crate::common::*;
        let mut events = open_reply();
        events.extend([block_stop(0), usage(Usage::default()), reply_end(json!("end_turn"))]);
        events
    }

    fn lines(tap_file: &std::path::Path) -> Vec<Value> {
        let text = std::fs::read_to_string(tap_file).unwrap();
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    async fn run_to_the_end(events: Vec<TapEvent>) -> Vec<Value> {
        run_recorded(events, |recorder| recorder).await
    }

    async fn run_recorded(events: Vec<TapEvent>, configure: impl FnOnce(Recorder) -> Recorder) -> Vec<Value> {
        let dir = tempfile::tempdir().unwrap();
        let tap_file = dir.path().join("tap.jsonl");
        let recorder = configure(Recorder::new(Sink::open(&tap_file).await.unwrap(), 1, "test"));
        let tap = ExchangeTap::spawn(recorder, Clock::start(), Box::new(Emits { events, release: None }));

        tap.send(TapInput::ResponseEnd);
        drop(tap);
        wait_for_exchange_end(&tap_file).await;

        lines(&tap_file)
    }

    #[tokio::test]
    async fn live_lines_come_before_one_record_before_exchange_end() {
        // It writes each live event, then the record when the reply ends
        let lines = run_to_the_end(closed_reply()).await;

        let kinds: Vec<&str> = lines.iter().map(|line| line["event"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "conversation",
                "block_start",
                "block_delta",
                "block_stop",
                "usage",
                "reply_end",
                "record",
                "exchange_end"
            ]
        );
        assert_eq!(lines[6]["payload"]["incomplete"], false);
        assert_eq!(lines[6]["payload"]["dropped"], 0);
    }

    #[tokio::test]
    async fn a_record_carries_the_ids_the_recorder_knows() {
        // It adds project, session and thread to the record and to no other line
        let known = Identity {
            session: Some("s1".into()),
            thread: Some("t1".into()),
            ..Identity::default()
        };

        let lines = run_recorded(closed_reply(), |recorder| {
            let recorder = recorder.knowing(known).awaiting_project(Duration::from_secs(5));
            recorder.project_looked_up(Some("host/owner/repo".into()));
            recorder
        })
        .await;

        let ids = |line: &Value| {
            [
                &line["payload"]["project"],
                &line["payload"]["session"],
                &line["payload"]["thread"],
            ]
            .map(Value::clone)
        };
        let record = lines.iter().find(|line| line["event"] == "record").unwrap();
        assert_eq!(ids(record), [json!("host/owner/repo"), json!("s1"), json!("t1")]);
        let others = lines.iter().filter(|line| line["event"] != "record");
        assert!(others.map(ids).all(|ids| ids.iter().all(Value::is_null)));
    }

    #[tokio::test]
    async fn a_record_waits_for_a_lookup_that_finishes_after_the_reply() {
        // It carries the project found while the record was already due
        let lines = run_recorded(closed_reply(), |recorder| {
            let recorder = recorder.awaiting_project(Duration::from_secs(5));
            let lookup = recorder.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                lookup.project_looked_up(Some("host/owner/repo".into()));
            });
            recorder
        })
        .await;

        let record = lines.iter().find(|line| line["event"] == "record").unwrap();
        assert_eq!(record["payload"]["project"], "host/owner/repo");
    }

    #[tokio::test]
    async fn a_record_stops_waiting_for_a_lookup_that_never_finishes() {
        // It is written without a project once the bound passes
        let lines = run_recorded(closed_reply(), |recorder| {
            recorder.awaiting_project(Duration::from_millis(20))
        })
        .await;

        let record = lines.iter().find(|line| line["event"] == "record").unwrap();
        assert!(record["payload"].get("project").is_none());
    }

    #[tokio::test]
    async fn a_lookup_that_finds_nothing_lets_the_record_go_without_waiting() {
        // It does not wait out the bound once the lookup has answered
        let started = Instant::now();

        let lines = run_recorded(closed_reply(), |recorder| {
            let recorder = recorder.awaiting_project(Duration::from_secs(30));
            recorder.project_looked_up(None);
            recorder
        })
        .await;

        let record = lines.iter().find(|line| line["event"] == "record").unwrap();
        assert!(record["payload"].get("project").is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_reliable_note_waits_for_room_in_a_full_queue() {
        // It delivers the line once the queue drains, where a plain note would drop it
        let (sink, mut drained) = Sink::detached(1);
        let recorder = Recorder::new(sink, 1, "test");
        let tap = ExchangeTap::spawn(
            recorder.clone(),
            Clock::start(),
            Box::new(Emits {
                events: Vec::new(),
                release: None,
            }),
        );
        recorder.write(Clock::start().stamp(), "filler", json!({}), (0, 0));
        let noted = tokio::spawn(async move { tap.note_reliable("edge", json!({"kind": "in_thread"})).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut kinds = Vec::new();
        while let Ok(Some(line)) = tokio::time::timeout(Duration::from_secs(2), drained.recv()).await {
            kinds.push(line["event"].as_str().unwrap().to_string());
            if line["event"] == "edge" {
                break;
            }
        }

        noted.await.unwrap();
        assert_eq!(kinds, ["filler", "edge"]);
    }

    #[tokio::test]
    async fn a_record_omits_the_ids_nobody_knows() {
        // It leaves the keys out instead of writing nulls
        let lines = run_to_the_end(closed_reply()).await;

        let record = lines.iter().find(|line| line["event"] == "record").unwrap();
        for key in ["project", "session", "thread"] {
            assert!(record["payload"].get(key).is_none());
        }
    }

    #[tokio::test]
    async fn closing_before_the_reply_ends_writes_one_incomplete_record() {
        // It keeps the assembled part and flags it
        let lines = run_to_the_end(open_reply()).await;

        let records: Vec<&Value> = lines.iter().filter(|line| line["event"] == "record").collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["payload"]["incomplete"], true);
        assert_eq!(records[0]["payload"]["reply"]["message"]["blocks"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn a_record_is_written_when_the_sink_refuses_live_lines_and_counts_them() {
        // It waits for room, so the record is not lost, and reports the refused live lines
        let (sink, mut drained) = Sink::detached(3);
        let recorder = Recorder::new(sink, 1, "test");
        let tap = ExchangeTap::spawn(
            recorder,
            Clock::start(),
            Box::new(Emits {
                events: closed_reply(),
                release: None,
            }),
        );
        tap.send(TapInput::ResponseEnd);
        drop(tap);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mut written = Vec::new();
        while let Some(line) = drained.recv().await {
            let last = line["event"] == "exchange_end";
            written.push(line);
            if last {
                break;
            }
        }

        let record = written.iter().find(|line| line["event"] == "record").unwrap();
        assert!(record["payload"]["dropped"].as_u64().unwrap() > 0);
        assert_eq!(record["payload"]["incomplete"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalled_parser_gives_an_incomplete_record_that_counts_the_dropped_input() {
        // It adds the inputs the full queue refused to the record's dropped count
        let dir = tempfile::tempdir().unwrap();
        let tap_file = dir.path().join("tap.jsonl");
        let recorder = Recorder::new(Sink::open(&tap_file).await.unwrap(), 1, "test");
        let (release, receiver) = std::sync::mpsc::channel();
        let tap = ExchangeTap::spawn(
            recorder,
            Clock::start(),
            Box::new(Emits {
                events: closed_reply(),
                release: Some(receiver),
            }),
        );

        for _ in 0..1000 {
            tap.send(TapInput::ResponseBody(Bytes::from_static(b"x")));
        }
        release.send(()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tap.send(TapInput::ResponseEnd);
        drop(tap);
        wait_for_exchange_end(&tap_file).await;

        let lines = lines(&tap_file);
        let record = lines.iter().find(|line| line["event"] == "record").unwrap();
        assert!(record["payload"]["dropped"].as_u64().unwrap() > 0);
        assert_eq!(record["payload"]["incomplete"], true);
    }
}
