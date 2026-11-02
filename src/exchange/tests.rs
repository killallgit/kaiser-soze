use super::recorder::Tracking;
use super::*;
use crate::adapter::event::*;
use crate::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::graph::Identity;
use crate::line::Line;
use crate::note::{self, Label, Note};
use crate::project::{self, ProjectCache, ProjectLookups};
use bytes::Bytes;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const ROOM: usize = 4096;
const WAIT: Duration = Duration::from_secs(10);

/// Exchanges writing to a sink nobody has to poll: `ended` reads the lines as they are written.
struct Capture {
    exchanges: Exchanges,
    drained: mpsc::Receiver<Line>,
    output: TapOutput,
}

impl Capture {
    fn new() -> Self {
        Self::finding_projects(ProjectLookups::new(ProjectCache::default()))
    }

    fn finding_projects(projects: ProjectLookups) -> Self {
        Self::holding(ROOM, projects)
    }

    fn with_room(room: usize) -> Self {
        Self::holding(room, ProjectLookups::new(ProjectCache::default()))
    }

    fn holding(room: usize, projects: ProjectLookups) -> Self {
        let (output, drained) = TapOutput::detached(room).unwrap();
        Self {
            exchanges: Exchanges::new(output.clone(), None, projects),
            drained,
            output,
        }
    }

    fn begin(
        &self,
        adapter: &dyn Adapter,
        label: Option<&Label>,
        parser: impl ExchangeParser + 'static,
    ) -> ExchangeTap {
        begin_with(&self.exchanges, adapter, label, Box::new(parser))
    }

    /// Begins an exchange whose reply ends with `events`, ends it, and returns the lines up to its `exchange_end`.
    async fn run(&mut self, adapter: &dyn Adapter, label: Option<&Label>, events: Vec<TapEvent>) -> Vec<Value> {
        let tap = self.begin(adapter, label, Emits { events, release: None });
        tap.send(TapInput::ResponseEnd);
        drop(tap);
        self.ended(1).await
    }

    /// The lines written up to the `count`th `exchange_end` from now.
    async fn ended(&mut self, count: usize) -> Vec<Value> {
        drain_ended(&mut self.drained, count).await
    }

    async fn wait_for_refusals(&self, count: u64) {
        let refused = async {
            while self.output.sink.dropped() < count {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(WAIT, refused)
            .await
            .expect("the sink refuses a line");
    }
}

async fn drain_ended(drained: &mut mpsc::Receiver<Line>, count: usize) -> Vec<Value> {
    let mut lines = Vec::new();
    let mut ends = 0;
    let all_ended = async {
        while ends < count {
            let line = drained.recv().await.expect("the sink stays open");
            ends += usize::from(line.entry.event() == "exchange_end");
            lines.push(serde_json::to_value(&line).unwrap());
        }
    };
    tokio::time::timeout(WAIT, all_ended)
        .await
        .expect("every exchange ends");
    lines
}

async fn run_to_the_end(adapter: &dyn Adapter, events: Vec<TapEvent>) -> Vec<Value> {
    Capture::new().run(adapter, None, events).await
}

fn record_of(lines: &[Value]) -> &Value {
    lines.iter().find(|line| line["event"] == "record").unwrap()
}

fn exchange_end_of(lines: &[Value]) -> &Value {
    lines.iter().find(|line| line["event"] == "exchange_end").unwrap()
}

struct BlocksUntilReleased {
    release: Receiver<()>,
    blocked: bool,
    messages_seen: Arc<Mutex<Vec<String>>>,
}

impl ExchangeParser for BlocksUntilReleased {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        if !std::mem::replace(&mut self.blocked, true) {
            self.release.recv().ok();
        }
        if let TapInput::WsMessage { text, .. } = input {
            self.messages_seen.lock().unwrap().push(text);
        }
        Vec::new()
    }
}

struct Fixture {
    capture: Capture,
    tap: ExchangeTap,
    release: std::sync::mpsc::Sender<()>,
    messages_seen: Arc<Mutex<Vec<String>>>,
}

fn blocked_tap() -> Fixture {
    let capture = Capture::new();
    let (release, receiver) = std::sync::mpsc::channel();
    let messages_seen = Arc::<Mutex<Vec<String>>>::default();
    let parser = BlocksUntilReleased {
        release: receiver,
        blocked: false,
        messages_seen: messages_seen.clone(),
    };
    let tap = capture.begin(&nobody(), None, parser);
    Fixture {
        capture,
        tap,
        release,
        messages_seen,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_parser_drops_and_counts_instead_of_blocking_the_sender() {
    // It counts what the full queue refused while the parser is blocked
    let Fixture {
        mut capture,
        tap,
        release,
        ..
    } = blocked_tap();

    for _ in 0..1000 {
        tap.send(TapInput::ResponseBody(Bytes::from_static(b"x")));
    }
    release.send(()).unwrap();
    drop(tap);

    let lines = capture.ended(1).await;
    assert!(exchange_end_of(&lines)["payload"]["tap_dropped"].as_u64().unwrap() > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_websocket_chunk_stops_decoding_only_from_the_gap() {
    // It decodes every chunk queued before the drop and none after it
    let Fixture {
        mut capture,
        tap,
        release,
        messages_seen,
    } = blocked_tap();
    let before_gap = Bytes::from_static(&[0x81, 0x01, b'a']);
    let after_gap = Bytes::from_static(&[0x81, 0x01, b'b']);

    for _ in 0..300 {
        tap.send_ws_bytes(true, before_gap.clone(), Duration::ZERO);
    }
    release.send(()).unwrap();
    // The queue has room again once the parser has drained it, so this frame is queued, not refused
    while messages_seen.lock().unwrap().len() < 256 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tap.send_ws_bytes(true, after_gap, Duration::ZERO);
    drop(tap);
    capture.ended(1).await;

    // The queue holds 256, plus the one the blocked parser had already taken
    let seen = messages_seen.lock().unwrap();
    assert!((256..=257).contains(&seen.len()));
    assert!(seen.iter().all(|text| text == "a"));
}

#[tokio::test]
async fn exchange_end_reports_records_the_sink_had_to_drop() {
    // It reports a non-zero sink_dropped when the sink queue overflowed
    let mut capture = Capture::with_room(1);
    let tap = capture.begin(
        &nobody(),
        None,
        Emits {
            events: closed_reply(),
            release: None,
        },
    );
    tap.send(TapInput::ResponseEnd);
    capture.wait_for_refusals(1).await;
    drop(tap);

    let lines = capture.ended(1).await;

    assert!(exchange_end_of(&lines)["payload"]["sink_dropped"].as_u64().unwrap() > 0);
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
    use crate::message::{Block, Message};
    vec![
        conversation(json!("sys"), json!([]), vec![Message::user(vec![Block::text("hi")])]),
        block_start(BlockStart {
            index: 0,
            kind: BlockKind::Text,
            id: Value::Null,
            name: Value::Null,
        }),
        block_delta(0, Delta::Text("ok".into())),
    ]
}

fn closed_reply() -> Vec<TapEvent> {
    use crate::common::*;
    let mut events = open_reply();
    events.extend([
        block_stop(0),
        usage(Usage::default()),
        reply_end_of_message(json!("end_turn"), Value::Null),
    ]);
    events
}

fn lines(tap_file: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(tap_file).unwrap();
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[tokio::test]
async fn a_record_of_an_exchange_without_a_label_carries_its_ids_and_no_project() {
    // It has no directory to find a project in, so the record names session, thread and turn only
    let who = Identifying(Identity {
        session: Some("s1".into()),
        thread: Some("t1".into()),
        turn: Some("u1".into()),
        ..Identity::default()
    });

    let lines = run_to_the_end(&who, closed_reply()).await;

    let record = &record_of(&lines)["payload"];
    assert_eq!(record["session"], "s1");
    assert_eq!(record["thread"], "t1");
    assert_eq!(record["turn"], "u1");
    assert!(record.get("project").is_none());
    assert_eq!(step_edges(&lines), [edge("turn", "step:1.1", "turn:u1")]);
}

#[tokio::test]
async fn live_lines_come_before_one_record_before_exchange_end() {
    // It writes each live event, then the record when the reply ends
    let lines = run_to_the_end(&nobody(), closed_reply()).await;

    let kinds: Vec<&str> = lines
        .iter()
        .map(|line| line["event"].as_str().unwrap())
        .filter(|kind| *kind != "exchange_start")
        .collect();
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
    assert_eq!(record_of(&lines)["payload"]["incomplete"], false);
    assert_eq!(record_of(&lines)["payload"]["dropped"], 0);
}

fn repository_with_origin(dir: &tempfile::TempDir) -> Label {
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        root.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://host/owner/repo.git\n",
    )
    .unwrap();
    labelled(&root.display().to_string())
}

fn locating_after(delay: Duration) -> ProjectLookups {
    let cache = ProjectCache::locating_with(move |dir| {
        std::thread::sleep(delay);
        project::locate(dir)
    });
    ProjectLookups::new(cache).with_wait(Duration::from_secs(5))
}

#[tokio::test]
async fn a_record_carries_the_ids_the_exchange_knows() {
    // It adds project, session and thread to the record and to no other line's payload
    let dir = tempfile::tempdir().unwrap();
    let label = repository_with_origin(&dir);

    let lines = Capture::finding_projects(locating_after(Duration::ZERO))
        .run(&in_session_and_thread(), Some(&label), closed_reply())
        .await;

    let ids = |line: &Value| {
        [
            &line["payload"]["project"],
            &line["payload"]["session"],
            &line["payload"]["thread"],
        ]
        .map(Value::clone)
    };
    assert_eq!(
        ids(record_of(&lines)),
        [json!("host/owner/repo"), json!("s1"), json!("t1")]
    );
    let others = lines.iter().filter(|line| line["event"] != "record");
    assert!(others.map(ids).all(|ids| ids.iter().all(Value::is_null)));
}

const STEPPED_LINES: [&str; 6] = [
    "conversation",
    "block_start",
    "block_delta",
    "block_stop",
    "usage",
    "reply_end",
];

fn steps_of_live_lines(lines: &[Value]) -> Vec<Option<u64>> {
    lines
        .iter()
        .filter(|line| STEPPED_LINES.contains(&line["event"].as_str().unwrap()))
        .map(|line| line.get("step").map(|step| step.as_u64().unwrap()))
        .collect()
}

#[tokio::test]
async fn live_lines_carry_the_step_of_the_record_they_are_folded_into() {
    // Each reply's live lines show the step of the record after them
    let mut second = closed_reply();
    second[0].event.conversation_mut().step = Some(2);
    let mut events = closed_reply();
    events[0].event.conversation_mut().step = Some(1);
    events.extend(second);

    let lines = run_to_the_end(&nobody(), events).await;

    let mut live_steps = Vec::new();
    let mut record_steps = Vec::new();
    for line in &lines {
        if line["event"] == "record" {
            record_steps.push(line["payload"]["step"].as_u64());
            live_steps.push(None);
        } else if STEPPED_LINES.contains(&line["event"].as_str().unwrap()) {
            live_steps.push(line["step"].as_u64());
        }
    }
    let per_reply: Vec<_> = live_steps
        .split(Option::is_none)
        .filter(|reply| !reply.is_empty())
        .collect();
    assert_eq!(record_steps, [Some(1), Some(2)]);
    assert_eq!(per_reply, [[Some(1); 6], [Some(2); 6]]);
}

#[tokio::test]
async fn a_conversation_with_no_step_gives_its_live_lines_step_one() {
    // A Claude request names no step, so the record is step 1 and so are its live lines
    let lines = run_to_the_end(&nobody(), closed_reply()).await;

    assert_eq!(steps_of_live_lines(&lines), [Some(1); 6]);
}

#[tokio::test]
async fn a_reply_with_no_conversation_has_no_step_anywhere() {
    // No request means no step, on the live lines and on the record
    let lines = run_to_the_end(&nobody(), closed_reply().split_off(1)).await;

    assert!(steps_of_live_lines(&lines).iter().all(Option::is_none));
    assert!(record_of(&lines)["payload"]["step"].is_null());
}

#[tokio::test]
async fn a_new_conversation_leaves_the_open_reply_on_its_old_step() {
    // The cut record and the lines before the new conversation show step 1; the new conversation shows step 2
    let mut events = open_reply();
    events[0].event.conversation_mut().step = Some(1);
    let mut next = closed_reply();
    next[0].event.conversation_mut().step = Some(2);
    events.extend(next);

    let lines = run_to_the_end(&nobody(), events).await;

    let steps = steps_of_live_lines(&lines);
    assert_eq!(&steps[..3], [Some(1); 3]);
    assert_eq!(&steps[3..], [Some(2); 6]);
    assert_eq!(record_of(&lines)["payload"]["step"], 1);
    assert_eq!(record_of(&lines)["payload"]["incomplete"], true);
}

#[tokio::test]
async fn every_line_of_an_identified_exchange_carries_its_session_and_thread() {
    // It puts the ids at the top of each line, and leaves the keys out when the exchange has none
    let identified = run_to_the_end(&in_session_and_thread(), closed_reply()).await;
    let anonymous = run_to_the_end(&nobody(), closed_reply()).await;

    assert!(identified
        .iter()
        .all(|line| line["session"] == "s1" && line["thread"] == "t1"));
    assert!(anonymous
        .iter()
        .all(|line| line.get("session").is_none() && line.get("thread").is_none()));
}

#[tokio::test]
async fn a_record_carries_the_turn_its_exchange_belongs_to() {
    // It groups replies of one user turn without saying the turn ended
    let who = Identifying(Identity {
        turn: Some("p1".into()),
        ..Identity::default()
    });

    let lines = run_to_the_end(&who, closed_reply()).await;

    assert_eq!(record_of(&lines)["payload"]["turn"], "p1");
    assert!(lines
        .iter()
        .filter(|line| line["event"] != "record")
        .all(|line| line["payload"].get("turn").is_none()));
}

#[tokio::test]
async fn a_record_takes_its_step_and_turn_from_its_own_request() {
    // A later Codex step names its own turn; a request that names none is step 1 of the header turn
    let who = Identifying(Identity {
        turn: Some("u1".into()),
        ..Identity::default()
    });
    let mut later = closed_reply();
    later[0].event.conversation_mut().step = Some(2);
    later[0].event.conversation_mut().turn = Some("u2".into());

    let stepped = run_to_the_end(&who, later).await;
    let plain = run_to_the_end(&who, closed_reply()).await;

    let step_and_turn = |lines: &[Value]| {
        let record = &record_of(lines)["payload"];
        (record["step"].clone(), record["turn"].clone())
    };
    assert_eq!(step_and_turn(&stepped), (json!(2), json!("u2")));
    assert_eq!(step_and_turn(&plain), (json!(1), json!("u1")));
}

#[tokio::test]
async fn a_record_writes_the_turn_and_continues_edges_of_its_step() {
    // A second exchange on the same thread continues the first, and both name their turn
    let who = Identifying(Identity {
        thread: Some("t1".into()),
        turn: Some("u1".into()),
        ..Identity::default()
    });
    let mut capture = Capture::new();

    let first = capture.run(&who, None, closed_reply()).await;
    let second = capture.run(&who, None, closed_reply()).await;

    assert_eq!(step_edges(&first), [edge("turn", "step:1.1", "turn:u1")]);
    assert_eq!(
        step_edges(&second),
        [
            edge("turn", "step:2.1", "turn:u1"),
            edge("continues", "step:2.1", "step:1.1")
        ]
    );
}

#[tokio::test]
async fn a_later_step_that_names_no_turn_does_not_take_the_header_turn() {
    // The header turn belongs to the first request of a socket; a later step may be a new turn
    let who = Identifying(Identity {
        turn: Some("u1".into()),
        ..Identity::default()
    });
    let mut later = closed_reply();
    later[0].event.conversation_mut().step = Some(2);

    let lines = run_to_the_end(&who, later).await;

    assert!(record_of(&lines)["payload"].get("turn").is_none());
    assert!(lines.iter().all(|line| line["payload"]["kind"] != "turn"));
}

#[tokio::test]
async fn a_result_in_a_later_request_writes_one_calls_edge_for_its_call() {
    // The first reply calls c1; the next request carries the result and the one after resends it
    use crate::common::*;
    use crate::message::{Block, Message, Output};
    let calling = vec![
        conversation(json!("sys"), json!([]), vec![Message::user(vec![Block::text("hi")])]),
        block_start(BlockStart {
            index: 0,
            kind: BlockKind::ToolCall,
            id: json!("c1"),
            name: json!("shell"),
        }),
        block_stop(0),
        reply_end_of_message(json!("tool_use"), Value::Null),
    ];
    let answering = |extra: &str| {
        let result = Message::user(vec![Block::ToolResult {
            call_id: json!("c1"),
            is_error: false,
            output: Output::Raw(json!("done")),
        }]);
        let mut events = closed_reply();
        events[0] = conversation(
            json!("sys"),
            json!([]),
            vec![result, Message::user(vec![Block::text(extra)])],
        );
        events
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
    let mut capture = Capture::new();
    let who = on_thread("t1");

    let first = capture.run(&who, None, calling).await;
    let second = capture.run(&who, None, answering("a")).await;
    let third = capture.run(&who, None, answering("b")).await;

    assert!(calls(&first).is_empty());
    assert_eq!(calls(&second), [("call:c1".to_string(), "result:c1".to_string())]);
    assert!(calls(&third).is_empty());
}

#[tokio::test]
async fn a_claude_and_a_codex_exchange_write_these_exact_records_and_edges() {
    // It pins every field of the record and edge payloads a thread's two exchanges write
    use crate::common::*;
    use crate::message::{Block, Message, Output};
    let claude = vec![
        conversation(json!("sys"), json!([]), vec![Message::user(vec![Block::text("hi")])]),
        block_start(BlockStart {
            index: 0,
            kind: BlockKind::ToolCall,
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
        vec![Message::user(vec![Block::ToolResult {
            call_id: json!("c1"),
            is_error: false,
            output: Output::Raw(json!("done")),
        }])],
    );
    codex[0].event.conversation_mut().step = Some(2);
    codex[0].event.conversation_mut().turn = Some("u2".into());
    codex[0].event.conversation_mut().previous_message_id = Some("m1".into());
    let who = Identifying(Identity {
        session: Some("s1".into()),
        thread: Some("t1".into()),
        turn: Some("u1".into()),
        ..Identity::default()
    });
    let no_usage = json!({"input": null, "output": null, "cache_read": null, "cache_write": null, "reasoning": null});
    let mut capture = Capture::new();

    let first = capture.run(&who, None, claude).await;
    let second = capture.run(&who, None, codex).await;

    assert_eq!(
        record_of(&first)["payload"],
        json!({
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
        })
    );
    assert_eq!(
        step_edge_payloads(&first),
        [json!({"kind": "turn", "from": "step:1.1", "to": "turn:u1"})]
    );
    assert_eq!(
        record_of(&second)["payload"],
        json!({
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
        })
    );
    assert_eq!(
        step_edge_payloads(&second),
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

/// The edges of a step, in the order they were written; the start edges are left out, as they race the record.
fn step_edge_payloads(lines: &[Value]) -> Vec<Value> {
    lines
        .iter()
        .filter(|line| line["event"] == "edge")
        .map(|line| line["payload"].clone())
        .filter(|payload| ["turn", "continues", "calls"].contains(&payload["kind"].as_str().unwrap()))
        .collect()
}

fn step_edges(lines: &[Value]) -> Vec<Edge> {
    step_edge_payloads(lines)
        .iter()
        .map(|payload| ["kind", "from", "to"].map(|key| payload[key].as_str().unwrap().to_string()))
        .map(|[kind, from, to]| (kind, from, to))
        .collect()
}

/// The step edges, sorted, so they say which edges were written and not in what order.
fn written_edges(lines: &[Value]) -> Vec<Edge> {
    let mut edges = step_edges(lines);
    edges.sort();
    edges
}

/// Begins one exchange of thread `t1` per entry of `replies` at once, all recording through one `Exchanges`.
/// The exchange numbered `late` (counting from 0) has its project lookup held back briefly, so either order can win.
async fn race_one_thread(replies: Vec<Vec<TapEvent>>, late: Option<usize>) -> Vec<Edge> {
    let slow_lookup = ProjectCache::locating_with(|_| {
        std::thread::sleep(Duration::from_millis(2));
        None
    });
    let mut capture = Capture::finding_projects(ProjectLookups::new(slow_lookup));
    let who = on_thread("t1");
    let count = replies.len();
    let mut taps = Vec::new();
    for (index, events) in replies.into_iter().enumerate() {
        let label = (late == Some(index)).then(|| labelled("/late"));
        let tap = capture.begin(&who, label.as_ref(), Emits { events, release: None });
        tap.send(TapInput::ResponseEnd);
        taps.push(tap);
    }
    drop(taps);

    written_edges(&capture.ended(count).await)
}

fn which_exchange_is_late(run: usize, exchanges: usize) -> Option<usize> {
    (run % (exchanges + 1)).checked_sub(1)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_claude_exchanges_of_one_thread_continue_by_message_id_however_the_first_record_is_held_up() {
    // Whichever exchange's record is held up, the step naming m1 continues the reply with m1 and pairs c1
    use crate::common::*;
    use crate::message::{Block, Message, Output};
    let calling = || {
        vec![
            conversation(json!("sys"), json!([]), vec![Message::user(vec![Block::text("hi")])]),
            block_start(BlockStart {
                index: 0,
                kind: BlockKind::ToolCall,
                id: json!("c1"),
                name: json!("shell"),
            }),
            block_stop(0),
            reply_end_of_message(json!("tool_use"), json!("m1")),
        ]
    };
    let answering = || {
        let result = Message::user(vec![Block::ToolResult {
            call_id: json!("c1"),
            is_error: false,
            output: Output::Raw(json!("done")),
        }]);
        let mut events = closed_reply();
        events[0] = conversation(json!("sys"), json!([]), vec![result]);
        events[0].event.conversation_mut().previous_message_id = Some("m1".into());
        events
    };
    let in_order = vec![
        edge("calls", "call:c1", "result:c1"),
        edge("continues", "step:2.1", "step:1.1"),
    ];

    for run in 0..50 {
        let edges = race_one_thread(vec![calling(), answering()], which_exchange_is_late(run, 2)).await;

        assert_eq!(edges, in_order, "run {run}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_exchanges_of_one_thread_form_one_chain_in_the_order_their_exchanges_began() {
    // With no previous message id each step continues the one before it, however the records are held up
    for run in 0..50 {
        let replies = vec![closed_reply(), closed_reply(), closed_reply()];
        let edges = race_one_thread(replies, which_exchange_is_late(run, 3)).await;
        let chain = vec![
            edge("continues", "step:2.1", "step:1.1"),
            edge("continues", "step:3.1", "step:2.1"),
        ];
        assert_eq!(edges, chain, "run {run}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exchange_that_ends_without_a_record_does_not_hold_back_its_threads_later_exchanges() {
    // It lets the later exchange write its record, which continues nothing as no step came before it
    let started = Instant::now();
    let edges = race_one_thread(vec![Vec::new(), closed_reply()], None).await;

    assert!(edges.is_empty());
    assert!(started.elapsed() < Duration::from_millis(1500));
}

async fn project_of_the_record_after_a_lookup_of(delay: Duration) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let label = repository_with_origin(&dir);

    let lines = Capture::finding_projects(locating_after(delay))
        .run(&nobody(), Some(&label), closed_reply())
        .await;

    record_of(&lines)["payload"]["project"].clone()
}

#[tokio::test]
async fn a_record_waits_for_a_lookup_that_finishes_after_the_reply() {
    // It carries the project found while the record was already due
    let project = project_of_the_record_after_a_lookup_of(Duration::from_millis(100)).await;

    assert_eq!(project, "host/owner/repo");
}

#[tokio::test]
async fn a_record_waits_for_as_long_as_its_lookup_takes() {
    // It carries a project that was settled long after the reply, as the lookup's own wait is the only bound
    let project = project_of_the_record_after_a_lookup_of(Duration::from_millis(300)).await;

    assert_eq!(project, "host/owner/repo");
}

#[tokio::test]
async fn a_record_carries_a_project_the_lookup_found_at_once() {
    // It leaves the answer alone once the lookup gave one
    let project = project_of_the_record_after_a_lookup_of(Duration::ZERO).await;

    assert_eq!(project, "host/owner/repo");
}

#[tokio::test]
async fn a_lookup_that_finds_nothing_lets_the_record_go_without_waiting() {
    // It does not wait out the bound once the lookup has answered
    let started = Instant::now();
    let lookups = ProjectLookups::new(ProjectCache::locating_with(|_| None)).with_wait(Duration::from_secs(5));

    let lines = Capture::finding_projects(lookups)
        .run(&nobody(), Some(&labelled("/nowhere")), closed_reply())
        .await;

    assert!(record_of(&lines)["payload"].get("project").is_none());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn a_record_is_written_without_a_project_when_its_signal_is_dropped_unanswered() {
    // It does not wait for a lookup that can no longer answer
    let (output, mut drained) = TapOutput::detached(ROOM).unwrap();
    let (recorder, signal) = Recorder::start(
        output,
        1,
        "test",
        Tracking {
            identity: Identity::default(),
            threads: None,
        },
    );
    drop(signal);
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

    let lines = drain_ended(&mut drained, 1).await;

    assert!(record_of(&lines)["payload"].get("project").is_none());
}

#[tokio::test]
async fn a_reliable_write_waits_for_room_in_a_full_queue() {
    // It delivers the start edge once the queue drains, where a plain write would drop it
    let mut capture = Capture::with_room(1);

    drop(capture.begin(
        &on_thread("t1"),
        None,
        Emits {
            events: Vec::new(),
            release: None,
        },
    ));
    let lines = capture.ended(1).await;

    let kinds: Vec<&str> = lines.iter().map(|line| line["event"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["exchange_start", "edge", "exchange_end"]);
    assert_eq!(exchange_end_of(&lines)["payload"]["sink_dropped"], 0);
}

#[tokio::test]
async fn a_record_omits_the_ids_nobody_knows() {
    // It leaves the keys out instead of writing nulls
    let lines = run_to_the_end(&nobody(), closed_reply()).await;

    for key in ["project", "session", "thread"] {
        assert!(record_of(&lines)["payload"].get(key).is_none());
    }
}

#[tokio::test]
async fn closing_before_the_reply_ends_writes_one_incomplete_record() {
    // It keeps the assembled part and flags it
    let lines = run_to_the_end(&nobody(), open_reply()).await;

    let records: Vec<&Value> = lines.iter().filter(|line| line["event"] == "record").collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["payload"]["incomplete"], true);
    assert_eq!(records[0]["payload"]["reply"]["message"]["blocks"][0]["text"], "ok");
}

#[tokio::test]
async fn a_record_is_written_when_the_sink_refuses_live_lines_and_counts_them() {
    // It waits for room, so the record is not lost, and reports the refused live lines
    let mut capture = Capture::with_room(1);
    let tap = capture.begin(
        &nobody(),
        None,
        Emits {
            events: closed_reply(),
            release: None,
        },
    );
    tap.send(TapInput::ResponseEnd);
    capture.wait_for_refusals(1).await;
    drop(tap);

    let lines = capture.ended(1).await;

    let record = &record_of(&lines)["payload"];
    assert!(record["dropped"].as_u64().unwrap() > 0);
    assert_eq!(record["incomplete"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_parser_gives_an_incomplete_record_that_counts_the_dropped_input() {
    // It adds the inputs the full queue refused to the record's dropped count
    let mut capture = Capture::new();
    let (release, receiver) = std::sync::mpsc::channel();
    let tap = capture.begin(
        &nobody(),
        None,
        Emits {
            events: closed_reply(),
            release: Some(receiver),
        },
    );

    for _ in 0..1000 {
        tap.send(TapInput::ResponseBody(Bytes::from_static(b"x")));
    }
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    tap.send(TapInput::ResponseEnd);
    drop(tap);

    let lines = capture.ended(1).await;

    let record = &record_of(&lines)["payload"];
    assert!(record["dropped"].as_u64().unwrap() > 0);
    assert_eq!(record["incomplete"], true);
}

pub(super) struct Identifying(Identity);

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

fn on_thread(thread: &str) -> Identifying {
    Identifying(Identity {
        thread: Some(thread.into()),
        ..Identity::default()
    })
}

pub(super) fn nobody() -> Identifying {
    Identifying(Identity::default())
}

fn labelled(dir: &str) -> Label {
    Label {
        command: "claude".into(),
        dir: dir.into(),
    }
}

fn begin_with(
    exchanges: &Exchanges,
    adapter: &dyn Adapter,
    label: Option<&Label>,
    parser: Box<dyn ExchangeParser>,
) -> ExchangeTap {
    let head = RequestHead {
        method: "POST".into(),
        host: "example.com".into(),
        path: "/v1/messages".into(),
        headers: Vec::new(),
    };
    exchanges.begin(Opening {
        adapter,
        head: &head,
        parser,
        label,
        http_version: hyper::Version::HTTP_11,
        upgrade: false,
    })
}

pub(super) fn begin(exchanges: &Exchanges, adapter: &dyn Adapter, label: Option<&Label>) -> ExchangeTap {
    let parser = Emits {
        events: Vec::new(),
        release: None,
    };
    begin_with(exchanges, adapter, label, Box::new(parser))
}

async fn exchanges_writing_to(dir: &tempfile::TempDir, projects: ProjectLookups) -> Exchanges {
    let output = TapOutput::open(&dir.path().join("tap.jsonl"), crate::logging::Logger::default())
        .await
        .unwrap();
    Exchanges::new(output, None, projects)
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
        Note::Tunnel(note::Tunnel {
            host: "h".into(),
            label: Some(labelled("/work")),
            port: 443,
        }),
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
    let mut capture = Capture::new();
    let heard = Arc::default();
    let tap = capture.begin(&nobody(), None, ReadsResponseHeads(Arc::clone(&heard)));
    tap.note_response_head(&response);
    drop(tap);

    let captured = capture.ended(1).await;
    let head_marked_arrived = exchange_end_of(&captured)["payload"]["response_head_us"].is_u64();
    let line = captured
        .into_iter()
        .find(|line| line["event"] == "response_head")
        .unwrap();
    let heard_by_parser = heard.lock().unwrap().clone();
    NotedHead {
        line,
        heard_by_parser,
        head_marked_arrived,
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
