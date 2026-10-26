use super::printer::PRINT_QUEUE;
use super::state::MAX_COLUMNS;
use super::*;
use crate::graph::{Edge, EdgeKind};
use crate::line::Entry;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

const T0: u64 = 43_421_000; // 12:03:41 UTC

fn tap(exchange: u64, adapter: &'static str, event: &str, wall_ms: u64, payload: Value) -> Line {
    Line {
        exchange,
        adapter,
        mono_us: 0,
        wall_ms,
        start: 0,
        end: 0,
        entry: Entry::untyped(event, payload),
    }
}

#[test]
fn an_edge_gives_no_live_line() {
    // It shows requests, replies and notes only, never the graph
    let edge = Line {
        entry: Entry::Edge(Edge::new(EdgeKind::InThread, "exchange:3", "thread:t1")),
        ..tap(3, "codex", "edge", T0, Value::Null)
    };

    assert_eq!(state().line(&edge), None);
}

fn state() -> LiveState {
    LiveState::new("/Users/me".into(), 0)
}

fn conversation(messages: usize, tools: usize) -> Value {
    json!({
        "system": [{"type": "text", "text": "SECRET-SYSTEM"}],
        "tools": (0..tools).map(|_| json!({"name": "SECRET-TOOL"})).collect::<Vec<_>>(),
        "messages": (0..messages)
            .map(|_| json!({"role": "user", "blocks": [{"kind": "text", "text": "SECRET-MESSAGE"}]}))
            .collect::<Vec<_>>(),
    })
}

fn record(blocks: Value, stop: Value) -> Value {
    json!({
        "conversation": conversation(1, 1),
        "reply": {
            "message": {"role": "assistant", "blocks": blocks},
            "usage": {"input": 1203, "output": 88},
            "stop_reason": stop,
        },
        "incomplete": false,
        "dropped": 0,
    })
}

#[test]
fn a_conversation_gives_one_request_line_with_counts() {
    let mut state = state();
    let line = state.line(&tap(3, "codex", "conversation", T0, conversation(7, 4)));

    assert_eq!(line.as_deref(), Some("12:03:41  #3 codex  -> 7 msgs  4 tools"));
}

#[test]
fn a_missing_tools_list_counts_as_zero_and_flags_are_shown() {
    let mut state = state();
    let mut payload = conversation(2, 0);
    payload["tools"] = Value::Null;
    payload["warmup"] = json!(true);
    payload["partial"] = json!(true);

    let line = state.line(&tap(3, "codex", "conversation", T0, payload));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #3 codex  -> 2 msgs  0 tools  warmup  partial")
    );
}

#[test]
fn the_request_line_ends_with_the_directory_from_the_exchange_label() {
    let mut state = state();
    let start = json!({"label": {"command": "claude", "dir": "/Users/me/Code/kaiser-soze"}});
    assert_eq!(state.line(&tap(3, "claude", "exchange_start", T0, start)), None);

    let line = state.line(&tap(3, "claude", "conversation", T0, conversation(1, 2)));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #3 claude  -> 1 msgs  2 tools  ~/Code/kaiser-soze")
    );
}

#[test]
fn a_record_gives_one_reply_line_with_elapsed_usage_and_block_kinds() {
    let mut state = state();
    state.line(&tap(3, "codex", "conversation", T0, conversation(1, 1)));
    let blocks = json!([
        {"kind": "text", "text": "SECRET-REPLY"},
        {"kind": "tool_call", "id": "c1", "name": "SECRET-TOOL", "input": {"cmd": "SECRET-INPUT"}}
    ]);

    let line = state.line(&tap(
        3,
        "codex",
        "record",
        T0 + 2_140,
        record(blocks, json!("completed")),
    ));

    assert_eq!(
        line.as_deref(),
        Some("12:03:43  #3 codex  <- completed  2.1s  in 1203 out 88  text,tool_call")
    );
}

#[test]
fn a_cut_off_record_shows_dashes_and_the_incomplete_and_dropped_notes() {
    let mut state = state();
    let mut payload = record(json!([]), Value::Null);
    payload["reply"]["usage"] = json!({"input": null, "output": null});
    payload["incomplete"] = json!(true);
    payload["dropped"] = json!(3);

    let line = state.line(&tap(4, "claude", "record", T0, payload));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #4 claude  <- -  in - out -  -  incomplete  dropped 3")
    );
}

#[test]
fn other_events_have_their_own_short_lines() {
    let mut state = state();
    let at = |event: &str, payload: Value| state_line(event, payload);
    fn state_line(event: &str, payload: Value) -> Option<String> {
        LiveState::new("/Users/me".into(), 0).line(&tap(5, "claude", event, T0, payload))
    }

    assert_eq!(
        at("unsupported", json!({"host": "h", "path": "/v1/count"})).as_deref(),
        Some("12:03:41  #5 claude  unsupported /v1/count")
    );
    assert_eq!(
        at("unparsed", json!({"side": "reply"})).as_deref(),
        Some("12:03:41  #5 claude  unparsed reply")
    );
    assert_eq!(
        at("error", json!({"error_type": "rate_limit"})).as_deref(),
        Some("12:03:41  #5 claude  error rate_limit")
    );
    let tunnel = json!({"host": "api2.cursor.sh", "port": 443,
        "label": {"command": "agent", "dir": "/Users/me/Code/x"}});
    assert_eq!(
        state.line(&tap(6, "none", "tunnel", T0, tunnel)).as_deref(),
        Some("12:03:41  #6 none  tunnel api2.cursor.sh:443  agent")
    );
}

#[test]
fn an_unsupported_path_keeps_two_parts_so_ids_deeper_down_stay_out_of_the_line() {
    let mut state = state();
    let payload = json!({"host": "h", "path": "/api/oauth/organizations/ORG-ID-123/plugins",
        "label": {"command": "claude", "dir": "/Users/me/p"}});

    let line = state.line(&tap(7, "claude", "unsupported", T0, payload));

    assert_eq!(
        line.as_deref(),
        Some("12:03:41  #7 claude  unsupported /api/oauth/…  claude")
    );
}

#[test]
fn streaming_and_bookkeeping_events_print_nothing() {
    let mut state = state();
    for event in [
        "block_start",
        "block_delta",
        "block_stop",
        "usage",
        "reply_end",
        "response_head",
        "exchange_start",
        "exchange_end",
    ] {
        assert_eq!(
            state.line(&tap(1, "codex", event, T0, json!({"text": "SECRET-DELTA"}))),
            None
        );
    }
}

#[test]
fn exchange_end_drops_that_exchanges_state() {
    let mut state = state();
    state.line(&tap(3, "codex", "conversation", T0, conversation(1, 1)));
    state.line(&tap(3, "codex", "exchange_end", T0, json!({})));

    let line = state.line(&tap(
        3,
        "codex",
        "record",
        T0 + 5_000,
        record(json!([]), json!("completed")),
    ));

    assert_eq!(
        line.as_deref(),
        Some("12:03:46  #3 codex  <- completed  in 1203 out 88  -")
    );
}

#[test]
fn no_line_contains_message_system_tool_or_reply_text() {
    let mut state = state();
    let blocks = json!([{"kind": "text", "text": "SECRET-REPLY"},
        {"kind": "tool_call", "id": "c1", "name": "SECRET-TOOL", "input": {"cmd": "SECRET-INPUT"}}]);
    let lines: Vec<String> = [
        tap(3, "codex", "conversation", T0, conversation(3, 2)),
        tap(3, "codex", "block_delta", T0, json!({"text": "SECRET-DELTA"})),
        tap(3, "codex", "record", T0 + 1, record(blocks, json!("completed"))),
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
    state.line(&tap(
        3,
        "claude",
        "exchange_start",
        T0,
        json!({"label": {"command": "claude", "dir": dir}}),
    ));

    let line = state
        .line(&tap(3, "claude", "conversation", T0, conversation(1, 1)))
        .unwrap();

    assert_eq!(line.chars().count(), MAX_COLUMNS);
    assert!(line.contains("  …"));
    assert!(line.ends_with("/project"));
}

#[test]
fn a_long_non_directory_line_is_cut_at_the_limit() {
    let mut state = state();
    let host = "h".repeat(200);

    let line = state
        .line(&tap(3, "none", "tunnel", T0, json!({"host": host, "port": 443})))
        .unwrap();

    assert_eq!(line.chars().count(), MAX_COLUMNS);
}

#[test]
fn the_time_uses_the_given_utc_offset() {
    let mut state = LiveState::new("/Users/me".into(), -5 * 3600);

    let line = state.line(&tap(1, "codex", "conversation", T0, conversation(1, 1)));

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
