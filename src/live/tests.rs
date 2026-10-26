use super::printer::PRINT_QUEUE;
use super::state::MAX_COLUMNS;
use super::*;
use crate::event::{
    BlockDelta, BlockKind, BlockStart, BlockStop, Conversation, Delta, Error, Event, ReplyEnd, Side, Unparsed, Usage,
};
use crate::graph::{Edge, EdgeKind};
use crate::label::Label;
use crate::line::Entry;
use crate::message::{Block, Message};
use crate::note::{ExchangeEnd, ExchangeStart, Note, ResponseHead, Transport, Tunnel, Unsupported};
use crate::record::Record;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

const T0: u64 = 43_421_000; // 12:03:41 UTC

fn line(exchange: u64, adapter: &'static str, wall_ms: u64, entry: Entry) -> Line {
    Line {
        exchange,
        adapter,
        mono_us: 0,
        wall_ms,
        start: 0,
        end: 0,
        entry,
    }
}

fn event(exchange: u64, adapter: &'static str, wall_ms: u64, event: Event) -> Line {
    line(exchange, adapter, wall_ms, Entry::Event(event))
}

fn note(exchange: u64, adapter: &'static str, wall_ms: u64, note: Note) -> Line {
    line(exchange, adapter, wall_ms, Entry::Note(note))
}

fn label(command: &str, dir: &str) -> Option<Label> {
    Some(Label {
        command: command.into(),
        dir: dir.into(),
    })
}

fn exchange_start(label: Option<Label>) -> Note {
    Note::ExchangeStart(ExchangeStart {
        headers: Vec::new(),
        host: "h".into(),
        http_version: "HTTP/1.1".into(),
        label,
        method: "POST".into(),
        path: "/".into(),
        upgrade: false,
    })
}

fn tunnel(host: &str, label: Option<Label>) -> Note {
    Note::Tunnel(Tunnel {
        host: host.into(),
        label,
        port: 443,
    })
}

fn unsupported(path: &str, label: Option<Label>) -> Note {
    Note::Unsupported(Unsupported {
        host: "h".into(),
        label,
        path: path.into(),
    })
}

#[test]
fn an_edge_gives_no_live_line() {
    // It shows requests, replies and notes only, never the graph
    let edge = Entry::Edge(Edge::new(EdgeKind::InThread, "exchange:3", "thread:t1"));

    assert_eq!(state().line(&line(3, "codex", T0, edge)), None);
}

fn state() -> LiveState {
    LiveState::new("/Users/me".into(), 0)
}

fn conversation(messages: usize, tools: usize) -> Conversation {
    Conversation::new(
        json!([{"type": "text", "text": "SECRET-SYSTEM"}]),
        Value::Array((0..tools).map(|_| json!({"name": "SECRET-TOOL"})).collect()),
        (0..messages)
            .map(|_| Message::user(vec![Block::text("SECRET-MESSAGE")]))
            .collect(),
    )
}

fn request_line(exchange: u64, adapter: &'static str, wall_ms: u64, conversation: Conversation) -> Line {
    event(exchange, adapter, wall_ms, Event::Conversation(conversation))
}

fn reply_line(exchange: u64, adapter: &'static str, wall_ms: u64, record: Record) -> Line {
    line(exchange, adapter, wall_ms, Entry::Record(Box::new(record)))
}

fn record(blocks: Vec<Block>, stop: Value) -> Record {
    let mut record = Record::empty();
    record.reply.message = Message::assistant(blocks);
    record.reply.usage.input = Some(1203);
    record.reply.usage.output = Some(88);
    record.reply.stop_reason = stop;
    record
}

fn text_and_tool_call() -> Vec<Block> {
    vec![
        Block::text("SECRET-REPLY"),
        Block::ToolCall {
            id: json!("c1"),
            input: json!({"cmd": "SECRET-INPUT"}),
            name: json!("SECRET-TOOL"),
        },
    ]
}

#[test]
fn a_conversation_gives_one_request_line_with_counts() {
    let mut state = state();
    let line = state.line(&request_line(3, "codex", T0, conversation(7, 4)));

    assert_eq!(line.as_deref(), Some("12:03:41  #3 codex  -> 7 msgs  4 tools"));
}

#[test]
fn a_missing_tools_list_counts_as_zero_and_flags_are_shown() {
    let mut state = state();
    let flagged = Conversation {
        tools: Value::Null,
        warmup: true,
        partial: true,
        ..conversation(2, 0)
    };

    let line = state.line(&request_line(3, "codex", T0, flagged));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #3 codex  -> 2 msgs  0 tools  warmup  partial")
    );
}

#[test]
fn the_request_line_ends_with_the_directory_from_the_exchange_label() {
    let mut state = state();
    let start = exchange_start(label("claude", "/Users/me/Code/kaiser-soze"));
    assert_eq!(state.line(&note(3, "claude", T0, start)), None);

    let line = state.line(&request_line(3, "claude", T0, conversation(1, 2)));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #3 claude  -> 1 msgs  2 tools  ~/Code/kaiser-soze")
    );
}

#[test]
fn a_record_gives_one_reply_line_with_elapsed_usage_and_block_kinds() {
    let mut state = state();
    state.line(&request_line(3, "codex", T0, conversation(1, 1)));

    let line = state.line(&reply_line(
        3,
        "codex",
        T0 + 2_140,
        record(text_and_tool_call(), json!("completed")),
    ));

    assert_eq!(
        line.as_deref(),
        Some("12:03:43  #3 codex  <- completed  2.1s  in 1203 out 88  text,tool_call")
    );
}

#[test]
fn a_cut_off_record_shows_dashes_and_the_incomplete_and_dropped_notes() {
    let mut state = state();
    let mut cut_off = record(Vec::new(), Value::Null);
    cut_off.reply.usage = Usage::default();
    cut_off.incomplete = true;
    cut_off.dropped = 3;

    let line = state.line(&reply_line(4, "claude", T0, cut_off));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #4 claude  <- -  in - out -  -  incomplete  dropped 3")
    );
}

#[test]
fn other_events_have_their_own_short_lines() {
    let mut state = state();
    let mut shown = |entry: Entry| state.line(&line(5, "claude", T0, entry));

    assert_eq!(
        shown(Entry::Note(unsupported("/v1/count", None))).as_deref(),
        Some("12:03:41  #5 claude  unsupported /v1/count")
    );
    assert_eq!(
        shown(Entry::Event(Event::Unparsed(Unparsed { side: Side::Reply }))).as_deref(),
        Some("12:03:41  #5 claude  unparsed reply")
    );
    let rate_limited = Error {
        error_type: json!("rate_limit"),
    };
    assert_eq!(
        shown(Entry::Event(Event::Error(rate_limited))).as_deref(),
        Some("12:03:41  #5 claude  error rate_limit")
    );
    let labelled = tunnel("api2.cursor.sh", label("agent", "/Users/me/Code/x"));
    assert_eq!(
        shown(Entry::Note(labelled)).as_deref(),
        Some("12:03:41  #5 claude  tunnel api2.cursor.sh:443  agent")
    );
}

#[test]
fn an_unsupported_path_keeps_two_parts_so_ids_deeper_down_stay_out_of_the_line() {
    let mut state = state();
    let deep = unsupported(
        "/api/oauth/organizations/ORG-ID-123/plugins",
        label("claude", "/Users/me/p"),
    );

    let line = state.line(&note(7, "claude", T0, deep));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #7 claude  unsupported /api/oauth/…  claude")
    );
}

fn quiet_entries() -> Vec<Entry> {
    let head = ResponseHead {
        headers: Vec::new(),
        http_version: "HTTP/1.1".into(),
        status: 200,
        transport: Transport::Sse,
    };
    let end = ExchangeEnd {
        bytes: 0,
        chunks: 0,
        first_byte_us: None,
        forward_max_us: 0,
        forward_sum_us: 0,
        response_head_us: None,
        sink_dropped: 0,
        tap_dropped: 0,
    };
    vec![
        Entry::Event(Event::BlockStart(BlockStart {
            id: Value::Null,
            index: 0,
            kind: BlockKind::Text,
            name: Value::Null,
        })),
        Entry::Event(Event::BlockDelta(BlockDelta {
            index: 0,
            delta: Delta::Text("SECRET-DELTA".into()),
        })),
        Entry::Event(Event::BlockStop(BlockStop { index: 0 })),
        Entry::Event(Event::Usage(Usage::default())),
        Entry::Event(Event::ReplyEnd(ReplyEnd {
            message_id: None,
            stop_reason: Value::Null,
        })),
        Entry::Note(Note::ResponseHead(head)),
        Entry::Note(exchange_start(None)),
        Entry::Note(Note::ExchangeEnd(end)),
    ]
}

#[test]
fn streaming_and_bookkeeping_events_print_nothing() {
    let mut state = state();
    for entry in quiet_entries() {
        assert_eq!(state.line(&line(1, "codex", T0, entry)), None);
    }
}

#[test]
fn exchange_end_drops_that_exchanges_state() {
    let mut state = state();
    let end = quiet_entries().pop().unwrap();
    state.line(&request_line(3, "codex", T0, conversation(1, 1)));
    state.line(&line(3, "codex", T0, end));

    let line = state.line(&reply_line(
        3,
        "codex",
        T0 + 5_000,
        record(Vec::new(), json!("completed")),
    ));

    assert_eq!(
        line.as_deref(),
        Some("12:03:46  #3 codex  <- completed  in 1203 out 88  -")
    );
}

#[test]
fn no_line_contains_message_system_tool_or_reply_text() {
    let mut state = state();
    let delta = Event::BlockDelta(BlockDelta {
        index: 0,
        delta: Delta::Text("SECRET-DELTA".into()),
    });
    let lines: Vec<String> = [
        request_line(3, "codex", T0, conversation(3, 2)),
        event(3, "codex", T0, delta),
        reply_line(3, "codex", T0 + 1, record(text_and_tool_call(), json!("completed"))),
    ]
    .iter()
    .filter_map(|line| state.line(line))
    .collect();

    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|line| !line.contains("SECRET")));
}

#[test]
fn a_long_directory_is_cut_from_the_left_to_fit_the_line() {
    let mut state = state();
    let dir = format!("/Users/me/{}/project", "deep/".repeat(40));
    state.line(&note(3, "claude", T0, exchange_start(label("claude", &dir))));

    let line = state.line(&request_line(3, "claude", T0, conversation(1, 1))).unwrap();

    assert_eq!(line.chars().count(), MAX_COLUMNS);
    assert!(line.contains("  …"));
    assert!(line.ends_with("/project"));
}

#[test]
fn a_long_non_directory_line_is_cut_at_the_limit() {
    let mut state = state();
    let host = "h".repeat(200);

    let line = state.line(&note(3, "none", T0, tunnel(&host, None))).unwrap();

    assert_eq!(line.chars().count(), MAX_COLUMNS);
}

#[test]
fn the_time_uses_the_given_utc_offset() {
    let mut state = LiveState::new("/Users/me".into(), -5 * 3600);

    let line = state.line(&request_line(1, "codex", T0, conversation(1, 1)));

    assert_eq!(line.as_deref(), Some("07:03:41  #1 codex  -> 1 msgs  1 tools"));
}

#[tokio::test]
async fn the_printer_writes_lines_in_order() {
    let (client, mut server) = tokio::io::duplex(4096);
    let printer = Printer::spawn(client);

    printer.offer("one".into());
    printer.offer("two".into());

    let mut seen = vec![0u8; 8];
    server.read_exact(&mut seen).await.unwrap();
    assert_eq!(seen, b"one\ntwo\n");
}

#[tokio::test]
async fn a_stuck_writer_never_blocks_offer_and_the_drops_are_counted_and_reported() {
    let (client, mut server) = tokio::io::duplex(4);
    let printer = Printer::spawn(client);

    for n in 0..(PRINT_QUEUE * 3) {
        printer.offer(format!("line {n}"));
    }
    assert!(printer.dropped() > 0);

    let dropped = printer.dropped();
    let mut all = Vec::new();
    let mut chunk = [0u8; 256];
    let seen = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let n = server.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            all.extend_from_slice(&chunk[..n]);
            if String::from_utf8_lossy(&all).contains("live lines dropped") {
                break;
            }
        }
    })
    .await;
    assert!(seen.is_ok());
    assert!(String::from_utf8_lossy(&all).contains(&format!("({dropped} live lines dropped)")));
}
