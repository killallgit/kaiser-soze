use crate::capture::MAX_BUFFER_BYTES;
use crate::event::{BlockDelta, BlockKind, BlockStart, Conversation, Delta, Event, Usage};
use crate::graph::claimed;
use crate::message::{Block, Message};
use crate::record::{Record, Reply};
use serde_json::{json, Value};
use std::collections::BTreeMap;

struct OpenBlock {
    kind: BlockKind,
    id: Value,
    name: Value,
    text: String,
    signature: Value,
    encrypted: Option<String>,
    input_json: String,
}

impl OpenBlock {
    fn into_block(self) -> Block {
        match self.kind {
            BlockKind::Reasoning => match self.encrypted {
                Some(encrypted) if self.text.is_empty() => Block::EncryptedReasoning {
                    encrypted: json!(encrypted),
                },
                _ => Block::reasoning(json!(self.text), self.signature),
            },
            BlockKind::ToolCall => Block::ToolCall {
                id: self.id,
                input: parse_input(&self.input_json),
                name: self.name,
            },
            _ => Block::text(self.text),
        }
    }
}

fn parse_input(joined: &str) -> Value {
    if joined.is_empty() {
        return json!({});
    }
    serde_json::from_str(joined).unwrap_or_else(|_| json!(joined))
}

#[derive(Default)]
struct OpenReply {
    blocks: BTreeMap<u64, OpenBlock>,
    usage: Usage,
    stop_reason: Value,
    message_id: Option<Value>,
    bytes: usize,
    truncated: bool,
    unparsed: bool,
}

/// Folds one exchange's common-format live events into finished records.
pub struct Assembler {
    bound: usize,
    conversation: Option<Conversation>,
    reply: OpenReply,
    seen: bool,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::with_bound(MAX_BUFFER_BYTES)
    }
}

impl Assembler {
    pub fn with_bound(bound: usize) -> Self {
        Self {
            bound,
            conversation: None,
            reply: OpenReply::default(),
            seen: false,
        }
    }

    /// Returns the records this event completes: one on a reply end, and one more when a new
    /// conversation arrives while a reply is still open.
    pub fn push(&mut self, event: &Event, dropped: u64) -> Vec<Record> {
        let mut records = Vec::new();
        match event {
            Event::Conversation(conversation) => {
                if self.seen {
                    records.push(self.take_record(dropped, true));
                }
                self.conversation = Some(conversation.clone());
            }
            Event::BlockStart(start) => self.start_block(start),
            Event::BlockDelta(delta) => self.add_delta(delta),
            Event::Usage(usage) => self.reply.usage.merge(usage.clone()),
            Event::ReplyEnd(end) => {
                self.reply.stop_reason = end.stop_reason.clone();
                self.reply.message_id = end.message_id.clone().filter(|id| !id.is_null());
                return vec![self.take_record(dropped, false)];
            }
            Event::Unparsed(_) => self.reply.unparsed = true,
            Event::BlockStop(_) | Event::Error(_) => {}
        }
        self.seen = true;
        records
    }

    /// The step of the record the open reply is folded into; none while no conversation opened it.
    pub fn step(&self) -> Option<u64> {
        self.conversation
            .as_ref()
            .map(|conversation| claimed(conversation).step)
    }

    /// Returns the open reply as an incomplete record, if any event was seen since the last record.
    pub fn finish(&mut self, dropped: u64) -> Option<Record> {
        self.seen.then(|| self.take_record(dropped, true))
    }

    fn start_block(&mut self, start: &BlockStart) {
        if start.kind == BlockKind::Other {
            return;
        }
        self.reply.blocks.insert(
            start.index,
            OpenBlock {
                kind: start.kind,
                id: start.id.clone(),
                name: start.name.clone(),
                text: String::new(),
                signature: Value::Null,
                encrypted: None,
                input_json: String::new(),
            },
        );
    }

    fn add_delta(&mut self, delta: &BlockDelta) {
        let Some(block) = self.reply.blocks.get_mut(&delta.index) else {
            return;
        };
        let piece = delta.delta.piece();
        if self.reply.truncated || self.reply.bytes + piece.len() > self.bound {
            self.reply.truncated = true;
            return;
        }
        self.reply.bytes += piece.len();
        match &delta.delta {
            Delta::Text(text) => block.text.push_str(text),
            Delta::InputJson(json) => block.input_json.push_str(json),
            Delta::Encrypted(encrypted) => block.encrypted = Some(encrypted.clone()),
            Delta::Signature(signature) => block.signature = json!(signature),
        }
    }

    fn take_record(&mut self, dropped: u64, cut: bool) -> Record {
        let reply = std::mem::take(&mut self.reply);
        let conversation = self.conversation.take();
        self.seen = false;
        let incomplete = cut || dropped > 0 || reply.unparsed || reply.truncated;
        let blocks = reply.blocks.into_values().map(OpenBlock::into_block).collect();
        let claim = conversation.as_ref().map(claimed);
        Record {
            conversation,
            dropped,
            incomplete,
            project: None,
            reply: Reply {
                message: Message::assistant(blocks),
                message_id: reply.message_id,
                stop_reason: reply.stop_reason,
                usage: reply.usage,
            },
            session: None,
            step: claim.as_ref().map(|claim| claim.step),
            thread: None,
            turn: claim.and_then(|claim| claim.turn),
        }
    }
}

#[cfg(test)]
mod tests;
