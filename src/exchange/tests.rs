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
