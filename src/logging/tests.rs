use super::formatting::{MAX_COLUMNS, MAX_RECORD_BYTES};
use super::*;
use crate::event::{BlockDelta, Delta, Event};
use crate::line::{Entry, Line};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader, DuplexStream};

async fn collected(log: Logger, mut reader: DuplexStream) -> Vec<Value> {
    drop(log);
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn json_settings(level: Level) -> Settings {
    Settings {
        level,
        format: Format::Json,
        ..Settings::default()
    }
}

fn delta() -> Line {
    Line {
        run_id: "0123456789abcdef0123456789abcdef".into(),
        sequence: 0,
        exchange: 7,
        adapter: "test",
        mono_us: 0,
        wall_ms: 10,
        start: 0,
        end: 99,
        session: None,
        thread: None,
        step: None,
        entry: Entry::Event(Event::BlockDelta(BlockDelta {
            index: 3,
            delta: Delta::Text("synthetic private body".repeat(100)),
        })),
    }
}

#[tokio::test]
async fn severity_filters_emit_typed_operational_records() {
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(writer, json_settings(Level::Warn));
    log.event(Level::Debug, "connection_opened", Fields::default());
    log.event(
        Level::Info,
        "listening",
        Fields {
            port: Some(48731),
            ..Fields::default()
        },
    );
    log.event(
        Level::Warn,
        "upstream_request_failed",
        Fields {
            exchange: Some(7),
            ..Fields::default()
        },
    );
    log.event(Level::Error, "sink_write_failed", Fields::default());

    let records = collected(log, reader).await;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["level"], "warn");
    assert_eq!(records[0]["event"], "upstream_request_failed");
    assert_eq!(records[0]["exchange"], 7);
    assert!(records[0]["timestamp_ms"].as_u64().is_some());
    assert_eq!(records[1]["level"], "error");
}

#[tokio::test]
async fn streaming_projects_only_bounded_metadata_from_a_delta() {
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(
        writer,
        Settings {
            stream: true,
            ..json_settings(Level::Debug)
        },
    );
    let line = delta();
    log.tap(&line);

    let records = collected(log, reader).await;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["event"], "block_delta");
    assert_eq!(record["exchange"], 7);
    assert_eq!(record["adapter"], "test");
    assert_eq!(record["run_id"], line.run_id.as_ref());
    assert_eq!(record["block"], 3);
    assert_eq!(record["bytes"], 2200);
    assert_eq!(record["field"], "text");
    let keys: Vec<&str> = record.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "adapter",
            "block",
            "bytes",
            "component",
            "event",
            "exchange",
            "field",
            "level",
            "run_id",
            "timestamp_ms"
        ]
    );
    assert!(record.to_string().len() <= MAX_RECORD_BYTES);
}

#[test]
fn a_stream_window_has_a_fixed_budget_and_resets_at_the_next_window() {
    let rate = Rate::default();
    for _ in 0..STREAM_PER_SECOND {
        assert!(rate.allow_at(0));
    }
    assert!(!rate.allow_at(0));
    assert!(rate.allow_at(1));
}

#[tokio::test]
async fn a_stalled_log_writer_drops_without_blocking_and_reports_typed_loss() {
    let (writer, reader) = tokio::io::duplex(1);
    let log = Logger::new(writer, json_settings(Level::Error));
    for _ in 0..LOG_QUEUE * 3 {
        log.event(Level::Error, "synthetic_failure", Fields::default());
    }
    assert!(log.dropped() > 0);

    let records = collected(log, reader).await;
    let loss = records.iter().find(|record| record["event"] == "log_dropped").unwrap();
    assert!(loss["dropped"].as_u64().unwrap() > 0);
    assert!(records
        .iter()
        .all(|record| record.to_string().len() <= MAX_RECORD_BYTES));
}

#[tokio::test]
async fn human_output_has_a_fixed_width_and_no_terminal_control_characters() {
    let (writer, mut reader) = tokio::io::duplex(8192);
    let log = Logger::new(writer, Settings::default());
    log.event(
        Level::Info,
        "ca_ready",
        Fields {
            ca_path: Some(format!("/tmp/{}\n\u{1b}[31m", "x".repeat(1000))),
            ..Fields::default()
        },
    );
    drop(log);
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_to_string(&mut text))
        .await
        .unwrap()
        .unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].chars().count() <= MAX_COLUMNS);
    assert!(lines[0].chars().all(|c| !c.is_control()));
}

#[tokio::test]
async fn a_failed_log_destination_never_keeps_a_producer_waiting() {
    let (writer, reader) = tokio::io::duplex(1);
    let log = Logger::new(writer, json_settings(Level::Info));
    drop(reader);
    log.event(Level::Error, "synthetic_failure", Fields::default());
    assert!(!log.flush().await);
    assert!(!log.event(Level::Error, "synthetic_failure", Fields::default()));
    assert!(log.dropped() > 0);
}

#[tokio::test]
async fn an_oversized_record_is_counted_and_later_records_still_arrive() {
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(writer, json_settings(Level::Info));
    let oversized: &'static str = "synthetic_event_".repeat(MAX_RECORD_BYTES).leak();
    log.event(Level::Error, oversized, Fields::default());
    log.event(Level::Info, "still_running", Fields::default());
    let records = collected(log, reader).await;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["event"], "log_dropped");
    assert_eq!(records[0]["dropped"], 1);
    assert_eq!(records[1]["event"], "still_running");
}

#[tokio::test]
async fn off_suppresses_all_levels_and_stream_metadata() {
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(
        writer,
        Settings {
            stream: true,
            ..json_settings(Level::Off)
        },
    );
    log.event(Level::Error, "synthetic_failure", Fields::default());
    log.tap(&delta());
    assert!(collected(log, reader).await.is_empty());
}

#[tokio::test]
async fn exchange_end_keeps_per_exchange_and_global_loss_counters_distinct() {
    use crate::note::{ExchangeEnd, Note};
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(writer, json_settings(Level::Warn));
    let mut line = delta();
    line.entry = Entry::Note(Note::ExchangeEnd(ExchangeEnd {
        bytes: 99,
        chunks: 2,
        first_byte_us: None,
        forward_max_us: 5,
        forward_sum_us: 8,
        response_head_us: Some(10),
        sink_dropped: 4,
        tap_dropped: 2,
    }));
    log.tap(&line);
    let records = collected(log, reader).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["tap_dropped"], 2);
    assert_eq!(records[0]["sink_dropped"], 4);
    assert!(records[0].get("dropped").is_none());
}

#[tokio::test]
async fn debug_file_resets_each_run_and_ignores_terminal_filters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("debug.jsonl");
    std::fs::write(&path, b"synthetic previous run\n").unwrap();
    let (terminal, reader) = tokio::io::duplex(8192);
    let log = Logger::new(terminal, json_settings(Level::Off))
        .with_debug_file(&path)
        .await
        .unwrap();
    for _ in 0..STREAM_PER_SECOND * 2 {
        log.tap(&delta());
    }
    assert!(log.flush().await);
    let records: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), (STREAM_PER_SECOND * 2) as usize);
    assert!(records.iter().all(|record| record["bytes"] == 2200));
    assert!(records.iter().all(|record| record.as_object().unwrap().len() == 10));
    assert!(collected(log, reader).await.is_empty());
}

#[tokio::test]
async fn a_stalled_terminal_does_not_block_the_debug_transport() {
    let (terminal, _blocked_reader) = tokio::io::duplex(1);
    let (file, file_reader) = tokio::io::duplex(8192);
    let terminal = Logger::new(terminal, json_settings(Level::Error));
    let debug = Logger::debug(file, terminal.clone(), 8192);
    let log = terminal.combine(debug);
    for _ in 0..LOG_QUEUE * 2 {
        log.event(Level::Error, "synthetic_failure", Fields::default());
    }
    let records = collected(log, file_reader).await;
    assert!(!records.is_empty());
}

#[tokio::test]
async fn a_stalled_debug_transport_does_not_block_terminal_records() {
    let (terminal, reader) = tokio::io::duplex(8192);
    let (file, blocked_reader) = tokio::io::duplex(1);
    let terminal = Logger::new(terminal, json_settings(Level::Error));
    let log = terminal.clone().combine(Logger::debug(file, terminal, 8192));
    for _ in 0..LOG_QUEUE * 2 {
        log.event(Level::Error, "synthetic_failure", Fields::default());
    }
    assert!(log.dropped() > 0);
    let mut lines = BufReader::new(reader).lines();
    let record = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let line = lines.next_line().await.unwrap().unwrap();
            let record: Value = serde_json::from_str(&line).unwrap();
            if record["event"] == "synthetic_failure" {
                return record;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(record["level"], "error");
    drop(blocked_reader);
}

#[tokio::test]
async fn a_debug_file_limit_stops_only_that_transport_and_reports_it() {
    let (terminal, reader) = tokio::io::duplex(8192);
    let (file, mut file_reader) = tokio::io::duplex(8192);
    let terminal = Logger::new(terminal, json_settings(Level::Warn));
    let log = terminal.clone().combine(Logger::debug(file, terminal, 512));
    for _ in 0..10 {
        log.tap(&delta());
    }
    assert!(!log.flush().await);
    let mut bytes = Vec::new();
    file_reader.read_to_end(&mut bytes).await.unwrap();
    assert!(bytes.len() <= 512);
    let lines: Vec<Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!lines.is_empty());
    log.event(Level::Error, "still_running", Fields::default());
    let records = collected(log, reader).await;
    assert!(records.iter().any(|record| record["event"] == "debug_file_limit"));
    assert!(records.iter().any(|record| record["event"] == "still_running"));
}

#[tokio::test]
async fn a_failed_debug_transport_reports_to_the_terminal() {
    let (terminal, reader) = tokio::io::duplex(8192);
    let (file, file_reader) = tokio::io::duplex(1);
    drop(file_reader);
    let terminal = Logger::new(terminal, json_settings(Level::Warn));
    let log = terminal.clone().combine(Logger::debug(file, terminal, 8192));
    log.tap(&delta());
    assert!(!log.flush().await);
    let records = collected(log, reader).await;
    let failure = records
        .iter()
        .find(|record| record["event"] == "debug_file_write_failed")
        .unwrap();
    assert_eq!(failure["error_kind"], "broken_pipe");
}

#[tokio::test]
async fn decisions_use_static_reasons_and_allowlisted_endpoints() {
    use crate::note::{Note, Unsupported};
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(
        writer,
        Settings {
            stream: true,
            ..json_settings(Level::Debug)
        },
    );
    let mut line = delta();
    line.entry = Entry::Note(Note::Unsupported(Unsupported {
        host: "api.anthropic.com".into(),
        label: None,
        path: "/v1/messages/count_tokens?secret=synthetic-private".into(),
    }));
    log.tap(&line);
    if let Entry::Note(Note::Unsupported(note)) = &mut line.entry {
        note.path = "/synthetic-private-path".into();
    }
    log.tap(&line);
    let records = collected(log, reader).await;
    assert_eq!(records[0]["decision"], "unsupported_endpoint");
    assert_eq!(records[0]["endpoint"], "/v1/messages/count_tokens");
    assert_eq!(records[1]["endpoint"], "other");
    assert!(records.iter().all(|record| record.as_object().unwrap().len() == 9));
}

#[tokio::test]
async fn conversation_counts_distinguish_roles_definitions_and_calls() {
    use crate::event::Conversation;
    use crate::message::{Block, Message, Role};
    let (writer, reader) = tokio::io::duplex(8192);
    let log = Logger::new(
        writer,
        Settings {
            live: true,
            ..json_settings(Level::Info)
        },
    );
    let mut line = delta();
    line.entry = Entry::Event(Event::Conversation(Conversation::new(
        serde_json::json!("synthetic system"),
        serde_json::to_value(vec![serde_json::json!({"name": "synthetic-tool"}); 17]).unwrap(),
        vec![
            Message {
                role: Role::from("system"),
                blocks: vec![Block::text("synthetic context")],
            },
            Message::user(vec![Block::text("synthetic prompt")]),
        ],
    )));
    log.tap(&line);
    let records = collected(log, reader).await;
    assert_eq!(records[0]["messages"], 2);
    assert_eq!(records[0]["tools"], 17);
    assert_eq!(records[0]["user_messages"], 1);
    assert_eq!(records[0]["system_messages"], 1);
    assert_eq!(records[0]["assistant_messages"], 0);
    assert_eq!(records[0]["other_messages"], 0);
    assert_eq!(records[0]["tool_calls"], 0);
}
