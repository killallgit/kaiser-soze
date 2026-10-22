use serde_json::Value;
use std::collections::HashMap;

pub(super) const MAX_COLUMNS: usize = 120;

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
                let path = short_path(payload["path"].as_str().unwrap_or("-"));
                (format!("unsupported {path}"), label_of(payload).0, None)
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
                let command = label_of(payload).0;
                let host = payload["host"].as_str().unwrap_or("-");
                (
                    format!("tunnel {host}:{}", payload["port"].as_u64().unwrap_or_default()),
                    command,
                    None,
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

/// The first two path parts. Deeper parts can carry account or organization ids.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match parts.len() {
        0 => "/".to_string(),
        1 | 2 => format!("/{}", parts.join("/")),
        _ => format!("/{}/…", parts[..2].join("/")),
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
