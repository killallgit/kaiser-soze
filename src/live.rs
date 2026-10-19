use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

const MAX_COLUMNS: usize = 120;
const PRINT_QUEUE: usize = 256;

#[derive(Default)]
struct Note {
    command: Option<String>,
    dir: Option<String>,
    requested_ms: Option<u64>,
}

/// Turns tap lines into the short live lines. Reads counts, kinds, flags and the label only,
/// never message, system or tool text.
pub struct LiveState {
    home: String,
    offset_secs: i64,
    exchanges: HashMap<u64, Note>,
}

impl LiveState {
    pub fn new(home: String, offset_secs: i64) -> Self {
        Self {
            home,
            offset_secs,
            exchanges: HashMap::new(),
        }
    }

    pub fn line(&mut self, tap: &Value) -> Option<String> {
        let exchange = tap["exchange"].as_u64()?;
        let wall = tap["wall_ms"].as_u64().unwrap_or_default();
        let payload = &tap["payload"];
        let (body, command, dir) = match tap["event"].as_str()? {
            "exchange_start" => {
                let note = self.exchanges.entry(exchange).or_default();
                (note.command, note.dir) = label_of(payload);
                return None;
            }
            "exchange_end" => {
                self.exchanges.remove(&exchange);
                return None;
            }
            "conversation" => {
                let note = self.exchanges.entry(exchange).or_default();
                note.requested_ms = Some(wall);
                let flag = |name: &str, shown: &str| {
                    if payload[name] == true {
                        format!("  {shown}")
                    } else {
                        String::new()
                    }
                };
                let body = format!(
                    "-> {} msgs  {} tools{}{}",
                    count(&payload["messages"]),
                    count(&payload["tools"]),
                    flag("warmup", "warmup"),
                    flag("partial", "partial"),
                );
                (body, None, note.dir.clone())
            }
            "record" => {
                let requested = self
                    .exchanges
                    .get_mut(&exchange)
                    .and_then(|note| note.requested_ms.take());
                (
                    reply_body(payload, requested.map(|at| wall.saturating_sub(at))),
                    None,
                    None,
                )
            }
            "unsupported" => {
                let (command, dir) = label_of(payload);
                (
                    format!("unsupported {}", payload["path"].as_str().unwrap_or("-")),
                    command,
                    dir,
                )
            }
            "unparsed" => (
                format!("unparsed {}", payload["side"].as_str().unwrap_or("-")),
                None,
                None,
            ),
            "error" => (
                format!("error {}", payload["error_type"].as_str().unwrap_or("-")),
                None,
                None,
            ),
            "tunnel" => {
                let (command, dir) = label_of(payload);
                let host = payload["host"].as_str().unwrap_or("-");
                (
                    format!("tunnel {host}:{}", payload["port"].as_u64().unwrap_or_default()),
                    command,
                    dir,
                )
            }
            _ => return None,
        };
        let head = format!(
            "{}  #{exchange} {}  {body}",
            self.clock(wall),
            tap["adapter"].as_str().unwrap_or("-")
        );
        Some(self.fit(head, command, dir))
    }

    fn clock(&self, wall_ms: u64) -> String {
        let secs = ((wall_ms / 1000) as i64 + self.offset_secs).rem_euclid(86_400);
        format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    }

    /// Appends the label and cuts the line to the column limit. The directory loses its left end first.
    fn fit(&self, mut head: String, command: Option<String>, dir: Option<String>) -> String {
        if let Some(command) = command {
            head = format!("{head}  {command}");
        }
        let Some(dir) = dir else {
            return head.chars().take(MAX_COLUMNS).collect();
        };
        let dir = match dir.strip_prefix(self.home.as_str()) {
            Some(rest) if !self.home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
            _ => dir,
        };
        let room = MAX_COLUMNS.saturating_sub(head.chars().count() + 2);
        let shown = if dir.chars().count() <= room {
            dir
        } else if room >= 4 {
            let tail: String = dir
                .chars()
                .rev()
                .take(room - 1)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("…{tail}")
        } else {
            return head.chars().take(MAX_COLUMNS).collect();
        };
        format!("{head}  {shown}").chars().take(MAX_COLUMNS).collect()
    }
}

fn count(list: &Value) -> usize {
    list.as_array().map_or(0, Vec::len)
}

fn label_of(payload: &Value) -> (Option<String>, Option<String>) {
    let text = |field: &str| payload["label"][field].as_str().map(String::from);
    (text("command"), text("dir"))
}

fn reply_body(record: &Value, elapsed_ms: Option<u64>) -> String {
    let reply = &record["reply"];
    let number = |value: &Value| value.as_u64().map_or("-".to_string(), |n| n.to_string());
    let kinds: Vec<&str> = reply["message"]["blocks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["kind"].as_str())
        .collect();
    let elapsed = elapsed_ms.map_or(String::new(), |ms| format!("  {:.1}s", ms as f64 / 1000.0));
    let mut body = format!(
        "<- {}{elapsed}  in {} out {}  {}",
        reply["stop_reason"].as_str().unwrap_or("-"),
        number(&reply["usage"]["input"]),
        number(&reply["usage"]["output"]),
        if kinds.is_empty() {
            "-".to_string()
        } else {
            kinds.join(",")
        },
    );
    if record["incomplete"] == true {
        body.push_str("  incomplete");
    }
    if let Some(dropped) = record["dropped"].as_u64().filter(|n| *n > 0) {
        body.push_str(&format!("  dropped {dropped}"));
    }
    body
}

pub struct Printer {
    tx: mpsc::Sender<String>,
    dropped: Arc<AtomicU64>,
}

impl Printer {
    pub fn spawn<W: AsyncWrite + Send + Unpin + 'static>(mut writer: W) -> Self {
        let (tx, mut rx) = mpsc::channel::<String>(PRINT_QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let counted = dropped.clone();
        tokio::spawn(async move {
            let mut reported = 0;
            while let Some(line) = rx.recv().await {
                let total = counted.load(Ordering::Relaxed);
                let mut out = String::new();
                if total > reported {
                    out = format!("({total} live lines dropped)\n");
                    reported = total;
                }
                out.push_str(&line);
                out.push('\n');
                if writer.write_all(out.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
        });
        Self { tx, dropped }
    }

    /// Queues a line, or drops and counts it when the queue is full. Never waits.
    pub fn offer(&self, line: String) {
        if self.tx.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// What the sink needs to show lines: the formatter and the printer.
pub struct Live {
    pub state: LiveState,
    pub printer: Printer,
}

impl Live {
    pub fn new<W: AsyncWrite + Send + Unpin + 'static>(writer: W, home: String, offset_secs: i64) -> Self {
        Self {
            state: LiveState::new(home, offset_secs),
            printer: Printer::spawn(writer),
        }
    }

    pub fn show(&mut self, tap: &Value) {
        if let Some(line) = self.state.line(tap) {
            self.printer.offer(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncReadExt;

    const T0: u64 = 43_421_000; // 12:03:41 UTC

    fn tap(exchange: u64, adapter: &str, event: &str, wall_ms: u64, payload: Value) -> Value {
        json!({"exchange": exchange, "adapter": adapter, "event": event, "wall_ms": wall_ms, "payload": payload})
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
            Some("12:03:41  #6 none  tunnel api2.cursor.sh:443  agent  ~/Code/x")
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
}
