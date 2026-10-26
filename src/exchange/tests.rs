use super::*;
use crate::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::graph::Identity;
use crate::graph::Threads;
use crate::label::Label;
use crate::line::Entry;
use crate::project::{ProjectCache, ProjectLookups};
use crate::sink::Sink;
use bytes::Bytes;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    wait_for_exchange_ends(tap_file, 1).await;
}

async fn wait_for_exchange_ends(tap_file: &std::path::Path, count: usize) {
    for _ in 0..50 {
        let text = std::fs::read_to_string(tap_file).unwrap_or_default();
        if text.matches("exchange_end").count() >= count {
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
        let recorder = recorder.knowing(known).awaiting_project();
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
async fn a_record_carries_the_turn_its_exchange_belongs_to() {
    // It groups replies of one user turn without saying the turn ended
    let known = Identity {
        turn: Some("p1".into()),
        ..Identity::default()
    };

    let lines = run_recorded(closed_reply(), |recorder| recorder.knowing(known)).await;

    let record = lines.iter().find(|line| line["event"] == "record").unwrap();
    assert_eq!(record["payload"]["turn"], "p1");
    assert!(lines
        .iter()
        .filter(|line| line["event"] != "record")
        .all(|line| line["payload"].get("turn").is_none()));
}

#[tokio::test]
async fn a_record_takes_its_step_and_turn_from_its_own_request() {
    // A later Codex step names its own turn; a request that names none is step 1 of the header turn
    let known = Identity {
        turn: Some("u1".into()),
        ..Identity::default()
    };
    let mut later = closed_reply();
    later[0].payload["step"] = json!(2);
    later[0].payload["turn"] = json!("u2");

    let stepped = run_recorded(later, |recorder| recorder.knowing(known.clone())).await;
    let plain = run_recorded(closed_reply(), |recorder| recorder.knowing(known)).await;

    let record = |lines: &[Value]| lines.iter().find(|line| line["event"] == "record").unwrap()["payload"].clone();
    assert_eq!(
        (record(&stepped)["step"].clone(), record(&stepped)["turn"].clone()),
        (json!(2), json!("u2"))
    );
    assert_eq!(
        (record(&plain)["step"].clone(), record(&plain)["turn"].clone()),
        (json!(1), json!("u1"))
    );
}

#[tokio::test]
async fn a_record_writes_the_turn_and_continues_edges_of_its_step() {
    // A second exchange on the same thread continues the first, and both name their turn
    let known = |turn: &str| Identity {
        thread: Some("t1".into()),
        turn: Some(turn.into()),
        ..Identity::default()
    };
    let threads = Arc::new(Mutex::new(Threads::default()));
    let edges = |lines: &[Value]| -> Vec<(String, String, String)> {
        lines
            .iter()
            .filter(|line| {
                line["event"] == "edge" && ["turn", "continues"].contains(&line["payload"]["kind"].as_str().unwrap())
            })
            .map(|line| ["kind", "from", "to"].map(|key| line["payload"][key].as_str().unwrap().to_string()))
            .map(|[kind, from, to]| (kind, from, to))
            .collect()
    };

    let first = run_recorded(closed_reply(), |r| r.knowing(known("u1")).tracking(threads.clone())).await;
    let second = run_recorded(closed_reply(), |mut r| {
        r.exchange = 2;
        r.knowing(known("u1")).tracking(threads.clone())
    })
    .await;

    assert_eq!(edges(&first), [("turn".into(), "step:1.1".into(), "turn:u1".into())]);
    assert_eq!(
        edges(&second),
        [
            ("turn".into(), "step:2.1".into(), "turn:u1".into()),
            ("continues".into(), "step:2.1".into(), "step:1.1".into())
        ]
    );
}

#[tokio::test]
async fn a_later_step_that_names_no_turn_does_not_take_the_header_turn() {
    // The header turn belongs to the first request of a socket; a later step may be a new turn
    let known = Identity {
        turn: Some("u1".into()),
        ..Identity::default()
    };
    let mut later = closed_reply();
    later[0].payload["step"] = json!(2);

    let lines = run_recorded(later, |recorder| {
        recorder
            .knowing(known)
            .tracking(Arc::new(Mutex::new(Threads::default())))
    })
    .await;

    let record = lines.iter().find(|line| line["event"] == "record").unwrap();
    assert!(record["payload"].get("turn").is_none());
    assert!(lines.iter().all(|line| line["payload"]["kind"] != "turn"));
}

#[tokio::test]
async fn a_result_in_a_later_request_writes_one_calls_edge_for_its_call() {
    // The first reply calls c1; the next request carries the result and the one after resends it
    use crate::common::*;
    let calling = vec![
        conversation(json!("sys"), json!([]), vec![message("user", vec![text_block("hi")])]),
        block_start(BlockStart {
            index: 0,
            kind: "tool_call",
            id: json!("c1"),
            name: json!("shell"),
        }),
        block_stop(0),
        reply_end(json!("tool_use")),
    ];
    let answering = |extra: &str| {
        let result = message("user", vec![tool_result_block(json!("c1"), json!("done"), false)]);
        let mut events = closed_reply();
        events[0] = conversation(
            json!("sys"),
            json!([]),
            vec![result, message("user", vec![text_block(extra)])],
        );
        events
    };
    let threads = Arc::new(Mutex::new(Threads::default()));
    let known = Identity {
        thread: Some("t1".into()),
        ..Identity::default()
    };
    let calls = |lines: &[Value]| -> Vec<(String, String)> {
        lines
            .iter()
            .filter(|line| line["payload"]["kind"] == "calls")
            .map(|line| {
                let end = |key: &str| line["payload"][key].as_str().unwrap().to_string();
                (end("from"), end("to"))
            })
            .collect()
    };
    let run = |exchange: u64, events: Vec<TapEvent>| {
        let (known, threads) = (known.clone(), threads.clone());
        run_recorded(events, move |mut r| {
            r.exchange = exchange;
            r.knowing(known).tracking(threads)
        })
    };

    let first = run(1, calling).await;
    let second = run(2, answering("a")).await;
    let third = run(3, answering("b")).await;

    assert!(calls(&first).is_empty());
    assert_eq!(calls(&second), [("call:c1".to_string(), "result:c1".to_string())]);
    assert!(calls(&third).is_empty());
}

#[tokio::test]
async fn a_claude_and_a_codex_exchange_write_these_exact_records_and_edges() {
    // It pins every field of the record and edge payloads a thread's two exchanges write
    use crate::common::*;
    let claude = vec![
        conversation(json!("sys"), json!([]), vec![message("user", vec![text_block("hi")])]),
        block_start(BlockStart {
            index: 0,
            kind: "tool_call",
            id: json!("c1"),
            name: json!("shell"),
        }),
        block_stop(0),
        usage(Usage::default()),
        reply_end_of_message(json!("tool_use"), json!("m1")),
    ];
    let mut codex = closed_reply();
    codex[0] = conversation(
        json!("sys"),
        json!([]),
        vec![message(
            "user",
            vec![tool_result_block(json!("c1"), json!("done"), false)],
        )],
    );
    codex[0].payload["step"] = json!(2);
    codex[0].payload["turn"] = json!("u2");
    codex[0].payload["previous_message_id"] = json!("m1");
    let threads = Arc::new(Mutex::new(Threads::default()));
    let known = Identity {
        session: Some("s1".into()),
        thread: Some("t1".into()),
        turn: Some("u1".into()),
        ..Identity::default()
    };
    let run = |exchange: u64, events: Vec<TapEvent>| {
        let (known, threads) = (known.clone(), threads.clone());
        run_recorded(events, move |mut r| {
            r.exchange = exchange;
            r.knowing(known).tracking(threads)
        })
    };
    let payloads = |lines: &[Value], event: &str| -> Vec<Value> {
        lines
            .iter()
            .filter(|line| line["event"] == event)
            .map(|line| line["payload"].clone())
            .collect()
    };
    let no_usage = json!({"input": null, "output": null, "cache_read": null, "cache_write": null, "reasoning": null});

    let first = run(1, claude).await;
    let second = run(2, codex).await;

    assert_eq!(
        payloads(&first, "record"),
        [json!({
            "conversation": {"system": "sys", "tools": [], "messages": [
                {"role": "user", "blocks": [{"kind": "text", "text": "hi"}]}
            ]},
            "reply": {
                "message": {"role": "assistant", "blocks": [
                    {"kind": "tool_call", "id": "c1", "name": "shell", "input": {}}
                ]},
                "usage": no_usage,
                "stop_reason": "tool_use",
                "message_id": "m1",
            },
            "incomplete": false,
            "dropped": 0,
            "step": 1,
            "turn": "u1",
            "session": "s1",
            "thread": "t1",
        })]
    );
    assert_eq!(
        payloads(&first, "edge"),
        [json!({"kind": "turn", "from": "step:1.1", "to": "turn:u1"})]
    );
    assert_eq!(
        payloads(&second, "record"),
        [json!({
            "conversation": {
                "system": "sys",
                "tools": [],
                "messages": [{"role": "user", "blocks": [
                    {"kind": "tool_result", "call_id": "c1", "output": "done", "is_error": false}
                ]}],
                "step": 2,
                "turn": "u2",
                "previous_message_id": "m1",
            },
            "reply": {
                "message": {"role": "assistant", "blocks": [{"kind": "text", "text": "ok"}]},
                "usage": no_usage,
                "stop_reason": "end_turn",
            },
            "incomplete": false,
            "dropped": 0,
            "step": 2,
            "turn": "u2",
            "session": "s1",
            "thread": "t1",
        })]
    );
    assert_eq!(
        payloads(&second, "edge"),
        [
            json!({"kind": "turn", "from": "step:2.2", "to": "turn:u2"}),
            json!({"kind": "continues", "from": "step:2.2", "to": "step:1.1"}),
            json!({"kind": "calls", "from": "call:c1", "to": "result:c1"}),
        ]
    );
}

type Edge = (String, String, String);

fn edge(kind: &str, from: &str, to: &str) -> Edge {
    (kind.into(), from.into(), to.into())
}

fn written_edges(lines: &[Value]) -> Vec<Edge> {
    let mut edges: Vec<Edge> = lines
        .iter()
        .filter(|line| line["event"] == "edge")
        .map(|line| ["kind", "from", "to"].map(|key| line["payload"][key].as_str().unwrap().to_string()))
        .map(|[kind, from, to]| (kind, from, to))
        .collect();
    edges.sort();
    edges
}

/// Runs one exchange of thread `t1` per entry of `replies` at once, all recording through one `Threads`.
/// The exchange numbered `late` (counting from 0) has its record held back briefly, so either order can win.
async fn race_one_thread(replies: Vec<Vec<TapEvent>>, late: Option<usize>) -> Vec<Edge> {
    let dir = tempfile::tempdir().unwrap();
    let tap_file = dir.path().join("tap.jsonl");
    let sink = Sink::open(&tap_file).await.unwrap();
    let threads = Arc::new(Mutex::new(Threads::default()));
    let known = Identity {
        thread: Some("t1".into()),
        ..Identity::default()
    };
    let count = replies.len();
    let mut taps = Vec::new();
    for (index, events) in replies.into_iter().enumerate() {
        let mut recorder = Recorder::new(sink.clone(), index as u64 + 1, "test")
            .knowing(known.clone())
            .tracking(threads.clone());
        if late == Some(index) {
            recorder = recorder.awaiting_project();
            let signal = recorder.project_signal();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(2)).await;
                signal.settle(None);
            });
        }
        let tap = ExchangeTap::spawn(recorder, Clock::start(), Box::new(Emits { events, release: None }));
        tap.send(TapInput::ResponseEnd);
        taps.push(tap);
    }
    drop(taps);
    wait_for_exchange_ends(&tap_file, count).await;

    written_edges(&lines(&tap_file))
}

fn which_exchange_is_late(run: usize, exchanges: usize) -> Option<usize> {
    (run % (exchanges + 1)).checked_sub(1)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_claude_exchanges_of_one_thread_continue_by_message_id_whichever_record_arrives_first() {
    // The step naming m1 continues the reply with m1 and pairs c1 when that reply came first; when it came
    // second, nothing is known of m1, and the reply without a previous id continues the last step
    use crate::common::*;
    let calling = || {
        vec![
            conversation(json!("sys"), json!([]), vec![message("user", vec![text_block("hi")])]),
            block_start(BlockStart {
                index: 0,
                kind: "tool_call",
                id: json!("c1"),
                name: json!("shell"),
            }),
            block_stop(0),
            reply_end_of_message(json!("tool_use"), json!("m1")),
        ]
    };
    let answering = || {
        let result = message("user", vec![tool_result_block(json!("c1"), json!("done"), false)]);
        let mut events = closed_reply();
        events[0] = conversation(json!("sys"), json!([]), vec![result]);
        events[0].payload["previous_message_id"] = json!("m1");
        events
    };
    let calling_first = vec![
        edge("calls", "call:c1", "result:c1"),
        edge("continues", "step:2.1", "step:1.1"),
    ];
    let answering_first = vec![edge("continues", "step:1.1", "step:2.1")];

    for run in 0..50 {
        let edges = race_one_thread(vec![calling(), answering()], which_exchange_is_late(run, 2)).await;

        assert!(
            edges == calling_first || edges == answering_first,
            "run {run} wrote neither order's edges: {edges:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_exchanges_of_one_thread_form_one_chain_in_the_order_their_records_arrive() {
    // With no previous message id each step continues the one before it, so no step continues itself or is continued twice
    for run in 0..50 {
        let replies = vec![closed_reply(), closed_reply(), closed_reply()];
        let edges = race_one_thread(replies, which_exchange_is_late(run, 3)).await;

        let continues: Vec<&Edge> = edges.iter().filter(|(kind, ..)| kind == "continues").collect();
        let distinct = |ends: Vec<&String>| ends.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(continues.len(), 2, "run {run}: {edges:?}");
        assert!(continues.iter().all(|(_, from, to)| from != to), "run {run}: {edges:?}");
        assert_eq!(
            distinct(continues.iter().map(|(_, from, _)| from).collect()),
            2,
            "run {run}: {edges:?}"
        );
        assert_eq!(
            distinct(continues.iter().map(|(_, _, to)| to).collect()),
            2,
            "run {run}: {edges:?}"
        );
        assert_eq!(edges.len(), 2, "run {run}: {edges:?}");
    }
}

#[tokio::test]
async fn a_record_waits_for_a_lookup_that_finishes_after_the_reply() {
    // It carries the project found while the record was already due
    let lines = run_recorded(closed_reply(), |recorder| {
        let recorder = recorder.awaiting_project();
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
async fn a_record_waits_for_as_long_as_its_lookup_takes() {
    // It carries a project that was settled long after the reply, as the lookup's own wait is the only bound
    let lines = run_recorded(closed_reply(), |recorder| {
        let recorder = recorder.awaiting_project();
        let signal = recorder.project_signal();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            signal.settle(Some("host/owner/repo".into()));
        });
        recorder
    })
    .await;

    let record = lines.iter().find(|line| line["event"] == "record").unwrap();
    assert_eq!(record["payload"]["project"], "host/owner/repo");
}

#[tokio::test]
async fn a_record_is_written_without_a_project_when_its_signal_is_dropped_unanswered() {
    // It does not wait for a lookup that can no longer answer
    let lines = run_recorded(closed_reply(), |recorder| {
        let recorder = recorder.awaiting_project();
        drop(recorder.project_signal());
        recorder
    })
    .await;

    let record = lines.iter().find(|line| line["event"] == "record").unwrap();
    assert!(record["payload"].get("project").is_none());
}

#[tokio::test]
async fn dropping_a_settled_signal_keeps_the_project() {
    // It leaves the answer alone once the lookup gave one
    let lines = run_recorded(closed_reply(), |recorder| {
        let recorder = recorder.awaiting_project();
        recorder.project_signal().settle(Some("host/owner/repo".into()));
        recorder
    })
    .await;

    let record = lines.iter().find(|line| line["event"] == "record").unwrap();
    assert_eq!(record["payload"]["project"], "host/owner/repo");
}

#[tokio::test]
async fn a_lookup_that_finds_nothing_lets_the_record_go_without_waiting() {
    // It does not wait out the bound once the lookup has answered
    let started = Instant::now();

    let lines = run_recorded(closed_reply(), |recorder| {
        let recorder = recorder.awaiting_project();
        recorder.project_looked_up(None);
        recorder
    })
    .await;

    let record = lines.iter().find(|line| line["event"] == "record").unwrap();
    assert!(record["payload"].get("project").is_none());
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn a_reliable_write_waits_for_room_in_a_full_queue() {
    // It delivers the line once the queue drains, where a plain write would drop it
    let (sink, mut drained) = Sink::detached(1);
    let recorder = Recorder::new(sink, 1, "test");
    recorder.write(Clock::start().stamp(), "filler", json!({}), (0, 0));
    let writer = recorder.clone();
    let written = tokio::spawn(async move {
        writer
            .write_reliable(Clock::start().stamp(), "edge", json!({"kind": "in_thread"}))
            .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut kinds = Vec::new();
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_secs(2), drained.recv()).await {
        kinds.push(line.entry.event().to_string());
        if line.entry.event() == "edge" {
            break;
        }
    }

    written.await.unwrap();
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
        let last = line.entry.event() == "exchange_end";
        written.push(line);
        if last {
            break;
        }
    }

    let record = written.iter().find(|line| line.entry.event() == "record").unwrap();
    let Entry::Untyped { payload, .. } = &record.entry else {
        panic!("a record is untyped");
    };
    assert!(payload["dropped"].as_u64().unwrap() > 0);
    assert_eq!(payload["incomplete"], true);
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

struct Identifying(Identity);

impl Adapter for Identifying {
    fn name(&self) -> &'static str {
        "test"
    }
    fn hosts(&self) -> &[String] {
        &[]
    }
    fn claim(&self, _head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        None
    }
    fn identity(&self, _head: &RequestHead) -> Identity {
        self.0.clone()
    }
}

fn in_session_and_thread() -> Identifying {
    Identifying(Identity {
        session: Some("s1".into()),
        thread: Some("t1".into()),
        ..Identity::default()
    })
}

fn nobody() -> Identifying {
    Identifying(Identity::default())
}

fn labelled(dir: &str) -> Label {
    Label {
        command: "claude".into(),
        dir: dir.into(),
    }
}

fn begin(exchanges: &Exchanges, adapter: &dyn Adapter, label: Option<&Label>) -> ExchangeTap {
    let head = RequestHead {
        method: "POST".into(),
        host: "example.com".into(),
        path: "/v1/messages".into(),
        headers: Vec::new(),
    };
    exchanges.begin(Opening {
        adapter,
        head: &head,
        parser: Box::new(Emits {
            events: Vec::new(),
            release: None,
        }),
        label,
        http_version: hyper::Version::HTTP_11,
        upgrade: false,
    })
}

async fn exchanges_writing_to(dir: &tempfile::TempDir, projects: ProjectLookups) -> Exchanges {
    let sink = Sink::open(&dir.path().join("tap.jsonl")).await.unwrap();
    Exchanges::new(sink, None, projects)
}

async fn written_lines(dir: &tempfile::TempDir, count: usize) -> Vec<Value> {
    let tap_file = dir.path().join("tap.jsonl");
    for _ in 0..100 {
        let text = std::fs::read_to_string(&tap_file).unwrap_or_default();
        if text.matches('\n').count() >= count {
            return lines(&tap_file);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the lines were never written");
}

fn kinds_of(lines: &[Value], exchange: u64) -> Vec<&str> {
    lines
        .iter()
        .filter(|line| line["exchange"] == exchange)
        .map(|line| line["event"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn exchanges_count_up_from_one_and_each_writes_its_start_then_its_edges_then_its_end() {
    // It ends an exchange only after the edges its start asked for, even when the tap is dropped at once
    let dir = tempfile::tempdir().unwrap();
    let exchanges = exchanges_writing_to(&dir, ProjectLookups::new(ProjectCache::default())).await;
    let who = in_session_and_thread();

    drop(begin(&exchanges, &who, None));
    drop(begin(&exchanges, &who, None));

    let lines = written_lines(&dir, 8).await;
    let expected = ["exchange_start", "edge", "edge", "exchange_end"];
    assert_eq!(kinds_of(&lines, 1), expected);
    assert_eq!(kinds_of(&lines, 2), expected);
}

#[tokio::test]
async fn a_hung_project_lookup_still_ends_the_exchange_with_the_edges_that_need_no_project() {
    // It writes in_session and in_thread, then exchange_end, once the lookup's wait passes
    let dir = tempfile::tempdir().unwrap();
    let (release, hung) = std::sync::mpsc::channel::<()>();
    let hung = Mutex::new(hung);
    let cache = ProjectCache::locating_with(move |_| {
        let _ = hung.lock().unwrap().recv();
        None
    });
    let lookups = ProjectLookups::new(cache).with_wait(Duration::from_millis(50));
    let exchanges = exchanges_writing_to(&dir, lookups).await;

    drop(begin(&exchanges, &in_session_and_thread(), Some(&labelled("/hangs"))));

    let lines = written_lines(&dir, 4).await;
    assert_eq!(kinds_of(&lines, 1), ["exchange_start", "edge", "edge", "exchange_end"]);
    let edge_kinds: Vec<&str> = lines
        .iter()
        .filter(|line| line["event"] == "edge")
        .map(|line| line["payload"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(edge_kinds, ["in_session", "in_thread"]);
    drop(release);
}

#[tokio::test]
async fn exchange_start_carries_the_request_and_the_label() {
    // It pins every field of the exchange_start payload
    let dir = tempfile::tempdir().unwrap();
    let exchanges = exchanges_writing_to(&dir, ProjectLookups::new(ProjectCache::default())).await;

    drop(begin(&exchanges, &nobody(), Some(&labelled("/work"))));

    let lines = written_lines(&dir, 2).await;
    assert_eq!(
        lines[0]["payload"],
        json!({
            "method": "POST",
            "host": "example.com",
            "path": "/v1/messages",
            "http_version": "HTTP/1.1",
            "upgrade": false,
            "headers": [],
            "label": {"command": "claude", "dir": "/work"},
        })
    );
}

#[tokio::test]
async fn a_note_alone_takes_the_next_exchange_id_and_carries_the_label() {
    // It shares the counter with exchanges and writes exactly one line
    let dir = tempfile::tempdir().unwrap();
    let exchanges = exchanges_writing_to(&dir, ProjectLookups::new(ProjectCache::default())).await;

    drop(begin(&exchanges, &nobody(), None));
    exchanges.note_alone(
        "none",
        "tunnel",
        json!({"host": "h", "port": 443}),
        Some(&labelled("/work")),
    );
    drop(begin(&exchanges, &nobody(), None));

    let lines = written_lines(&dir, 5).await;
    assert_eq!(kinds_of(&lines, 2), ["tunnel"]);
    let tunnel = lines.iter().find(|line| line["event"] == "tunnel").unwrap();
    assert_eq!(tunnel["adapter"], "none");
    assert_eq!(
        tunnel["payload"],
        json!({"host": "h", "port": 443, "label": {"command": "claude", "dir": "/work"}})
    );
    assert_eq!(kinds_of(&lines, 3), ["exchange_start", "exchange_end"]);
}

type Heads = Vec<(u16, Vec<(String, String)>)>;

struct ReadsResponseHeads(Arc<Mutex<Heads>>);

impl ExchangeParser for ReadsResponseHeads {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        if let TapInput::ResponseHead(head) = input {
            self.0.lock().unwrap().push((head.status, head.headers));
        }
        Vec::new()
    }
}

struct NotedHead {
    line: Value,
    heard_by_parser: Heads,
    head_marked_arrived: bool,
}

async fn note_response_head(response: hyper::Response<()>) -> NotedHead {
    let dir = tempfile::tempdir().unwrap();
    let tap_file = dir.path().join("tap.jsonl");
    let recorder = Recorder::new(Sink::open(&tap_file).await.unwrap(), 1, "test");
    let heard = Arc::default();
    let tap = ExchangeTap::spawn(
        recorder,
        Clock::start(),
        Box::new(ReadsResponseHeads(Arc::clone(&heard))),
    );
    let stats = tap.stats.clone();

    tap.note_response_head(&response);
    drop(tap);
    wait_for_exchange_end(&tap_file).await;

    let line = lines(&tap_file)
        .into_iter()
        .find(|line| line["event"] == "response_head")
        .unwrap();
    let heard_by_parser = heard.lock().unwrap().clone();
    NotedHead {
        line,
        heard_by_parser,
        head_marked_arrived: stats.head_us.load(Ordering::Relaxed) != stats::UNSET,
    }
}

fn response(status: u16, headers: &[(&str, &str)]) -> hyper::Response<()> {
    let mut builder = hyper::Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(()).unwrap()
}

#[tokio::test]
async fn a_response_head_is_written_with_redacted_headers_and_handed_to_the_parser() {
    // It writes the status, version, transport and headers, and the parser gets the same status and headers
    let noted = note_response_head(response(
        200,
        &[("content-type", "text/event-stream"), ("set-cookie", "id=1")],
    ))
    .await;

    let headers = json!([["content-type", "text/event-stream"], ["set-cookie", "[redacted]"]]);
    assert_eq!(
        noted.line["payload"],
        json!({"status": 200, "http_version": "HTTP/1.1", "transport": "sse", "headers": headers})
    );
    let headers: Vec<(String, String)> = serde_json::from_value(headers).unwrap();
    assert_eq!(noted.heard_by_parser, [(200, headers)]);
    assert!(noted.head_marked_arrived);
}

#[tokio::test]
async fn a_response_head_names_its_transport() {
    // It says websocket for a switch of protocols, sse for an event stream and http otherwise
    let websocket = note_response_head(response(101, &[])).await;
    let stream = note_response_head(response(200, &[("content-type", "text/event-stream; charset=utf-8")])).await;
    let plain = note_response_head(response(200, &[("content-type", "application/json")])).await;

    let transport = |noted: &NotedHead| noted.line["payload"]["transport"].clone();
    assert_eq!(
        [transport(&websocket), transport(&stream), transport(&plain)],
        [json!("websocket"), json!("sse"), json!("http")]
    );
}
