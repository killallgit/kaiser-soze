use super::Fields;
use crate::event::{Delta, Event};
use crate::line::{Entry, Line};
use crate::message::{Block, Role};
use crate::note::Note;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Warning,
    Summary,
    Stream,
}

pub(super) fn kind(entry: &Entry) -> Kind {
    if matches!(entry, Entry::Event(Event::Error(_) | Event::Unparsed(_)))
        || matches!(entry, Entry::Record(record) if record.incomplete)
        || matches!(entry, Entry::Note(Note::ExchangeEnd(end)) if end.tap_dropped.saturating_add(end.sink_dropped) > 0)
    {
        Kind::Warning
    } else if matches!(entry, Entry::Event(Event::Conversation(_)) | Entry::Record(_)) {
        Kind::Summary
    } else {
        Kind::Stream
    }
}

pub(super) fn fields(line: &Line) -> Fields {
    let mut fields = Fields {
        exchange: Some(line.exchange),
        adapter: Some(line.adapter),
        ..Fields::default()
    };
    match &line.entry {
        Entry::Event(Event::Conversation(value)) => {
            fields.messages = Some(value.messages.len());
            fields.tools = value.tools.as_array().map(Vec::len);
            let user = Role::from("user");
            let assistant = Role::from("assistant");
            let system = Role::from("system");
            let users = value.messages.iter().filter(|m| m.role == user).count();
            let assistants = value.messages.iter().filter(|m| m.role == assistant).count();
            let systems = value.messages.iter().filter(|m| m.role == system).count();
            fields.user_messages = Some(users);
            fields.assistant_messages = Some(assistants);
            fields.system_messages = Some(systems);
            fields.other_messages = Some(value.messages.len() - users - assistants - systems);
            fields.tool_calls = Some(
                value
                    .messages
                    .iter()
                    .flat_map(|m| &m.blocks)
                    .filter(|b| matches!(b, Block::ToolCall { .. }))
                    .count(),
            );
            fields.step = value.step;
        }
        Entry::Event(Event::BlockDelta(value)) => {
            fields.block = Some(value.index);
            fields.bytes = Some(value.delta.piece().len() as u64);
            fields.field = Some(match value.delta {
                Delta::Text(_) => "text",
                Delta::InputJson(_) => "input_json",
                Delta::Signature(_) => "signature",
                Delta::Encrypted(_) => "encrypted",
            });
        }
        Entry::Event(Event::BlockStart(value)) => fields.block = Some(value.index),
        Entry::Event(Event::BlockStop(value)) => fields.block = Some(value.index),
        Entry::Event(Event::Usage(value)) => {
            fields.input_tokens = value.input;
            fields.output_tokens = value.output;
        }
        Entry::Record(value) => {
            fields.blocks = Some(value.reply.message.blocks.len());
            fields.input_tokens = value.reply.usage.input;
            fields.output_tokens = value.reply.usage.output;
            fields.incomplete = Some(value.incomplete);
            fields.dropped = Some(value.dropped);
            fields.step = value.step;
        }
        Entry::Note(Note::ResponseHead(value)) => fields.status = Some(value.status),
        Entry::Note(Note::ExchangeStart(value)) => {
            fields.decision = Some("parsed_endpoint");
            fields.endpoint = Some(endpoint(&value.path));
        }
        Entry::Note(Note::Unsupported(value)) => {
            fields.decision = Some("unsupported_endpoint");
            fields.endpoint = Some(endpoint(&value.path));
        }
        Entry::Note(Note::Tunnel(value)) => {
            fields.decision = Some("unclaimed_host_passthrough");
            fields.port = Some(value.port);
        }
        Entry::Note(Note::ExchangeEnd(value)) => {
            fields.bytes = Some(value.bytes);
            fields.tap_dropped = Some(value.tap_dropped);
            fields.sink_dropped = Some(value.sink_dropped);
            fields.response_head_us = value.response_head_us;
            fields.chunks = Some(value.chunks);
            fields.forward_max_us = Some(value.forward_max_us);
        }
        _ => {}
    }
    fields
}

// Only known API routes can enter operational logs. Arbitrary paths and queries may carry secrets.
fn endpoint(path: &str) -> &'static str {
    match path.split('?').next().unwrap_or_default() {
        "/v1/messages" => "/v1/messages",
        "/v1/messages/count_tokens" => "/v1/messages/count_tokens",
        "/v1/responses" => "/v1/responses",
        "/backend-api/codex/responses" => "/backend-api/codex/responses",
        "/api/event_logging/batch" => "/api/event_logging/batch",
        _ => "other",
    }
}
