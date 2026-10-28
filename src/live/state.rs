use crate::event::{Conversation, Event};
use crate::line::{Entry, Line};
use crate::message::Block;
use crate::note::{Label, Note};
use crate::record::Record;
use std::collections::HashMap;

/// A line's text, then the command and directory it ends with.
type Shown = (String, Option<String>, Option<String>);

pub(super) const MAX_COLUMNS: usize = 120;

#[derive(Default)]
struct OpenExchange {
    command: Option<String>,
    dir: Option<String>,
    requested_ms: Option<u64>,
}

/// Turns tap lines into the short live lines. Reads counts, kinds, flags and the label only,
/// never message, system or tool text.
pub struct LiveState {
    home: String,
    offset_secs: i64,
    exchanges: HashMap<u64, OpenExchange>,
}

impl LiveState {
    pub fn new(home: String, offset_secs: i64) -> Self {
        Self {
            home,
            offset_secs,
            exchanges: HashMap::new(),
        }
    }

    pub fn line(&mut self, tap: &Line) -> Option<String> {
        let (body, command, dir) = match &tap.entry {
            Entry::Event(event) => self.event_body(tap, event)?,
            Entry::Note(note) => self.note_body(tap, note)?,
            Entry::Record(record) => (self.record_body(tap, record), None, None),
            Entry::Edge(_) => return None,
        };
        let (exchange, wall) = (tap.exchange, tap.wall_ms);
        let head = format!("{}  #{exchange} {}  {body}", self.clock(wall), tap.adapter);
        Some(self.fit(head, command, dir))
    }

    fn event_body(&mut self, tap: &Line, event: &Event) -> Option<Shown> {
        Some(match event {
            Event::Conversation(conversation) => {
                let open = self.exchanges.entry(tap.exchange).or_default();
                open.requested_ms = Some(tap.wall_ms);
                (request_body(conversation), None, open.dir.clone())
            }
            Event::Unparsed(unparsed) => (format!("unparsed {}", unparsed.side.as_str()), None, None),
            Event::Error(error) => (
                format!("error {}", error.error_type.as_str().unwrap_or("-")),
                None,
                None,
            ),
            _ => return None,
        })
    }

    fn note_body(&mut self, tap: &Line, note: &Note) -> Option<Shown> {
        Some(match note {
            Note::ExchangeStart(start) => {
                let open = self.exchanges.entry(tap.exchange).or_default();
                (open.command, open.dir) = label_of(start.label.as_ref());
                return None;
            }
            Note::ExchangeEnd(_) => {
                self.exchanges.remove(&tap.exchange);
                return None;
            }
            Note::Unsupported(unsupported) => (
                format!("unsupported {}", short_path(&unsupported.path)),
                label_of(unsupported.label.as_ref()).0,
                None,
            ),
            Note::Tunnel(tunnel) => (
                format!("tunnel {}:{}", tunnel.host, tunnel.port),
                label_of(tunnel.label.as_ref()).0,
                None,
            ),
            Note::ResponseHead(_) => return None,
        })
    }

    fn record_body(&mut self, tap: &Line, record: &Record) -> String {
        let requested = self
            .exchanges
            .get_mut(&tap.exchange)
            .and_then(|open| open.requested_ms.take());
        reply_body(record, requested.map(|at| tap.wall_ms.saturating_sub(at)))
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

fn request_body(conversation: &Conversation) -> String {
    let flag = |set: bool, shown: &str| if set { format!("  {shown}") } else { String::new() };
    format!(
        "-> {} msgs  {} tools{}{}",
        conversation.messages.len(),
        conversation.tools.as_array().map_or(0, Vec::len),
        flag(conversation.warmup, "warmup"),
        flag(conversation.partial, "partial"),
    )
}

fn label_of(label: Option<&Label>) -> (Option<String>, Option<String>) {
    (
        label.map(|label| label.command.clone()),
        label.map(|label| label.dir.clone()),
    )
}

fn reply_body(record: &Record, elapsed_ms: Option<u64>) -> String {
    let reply = &record.reply;
    let number = |count: Option<u64>| count.map_or("-".to_string(), |n| n.to_string());
    let kinds: Vec<&str> = reply.message.blocks.iter().map(Block::kind).collect();
    let elapsed = elapsed_ms.map_or(String::new(), |ms| format!("  {:.1}s", ms as f64 / 1000.0));
    let mut body = format!(
        "<- {}{elapsed}  in {} out {}  {}",
        reply.stop_reason.as_str().unwrap_or("-"),
        number(reply.usage.input),
        number(reply.usage.output),
        if kinds.is_empty() {
            "-".to_string()
        } else {
            kinds.join(",")
        },
    );
    if record.incomplete {
        body.push_str("  incomplete");
    }
    if record.dropped > 0 {
        body.push_str(&format!("  dropped {}", record.dropped));
    }
    body
}
