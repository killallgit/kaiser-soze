use crate::adapter::TapEvent;
use crate::body::MAX_REQUEST_BODY;
use crate::common::{encrypted_reasoning_block, message, reasoning_block, text_block, tool_call_block};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

const USAGE_FIELDS: [&str; 5] = ["input", "output", "cache_read", "cache_write", "reasoning"];

struct Block {
    kind: String,
    id: Value,
    name: Value,
    text: String,
    signature: Value,
    encrypted: Option<String>,
    input_json: String,
}

impl Block {
    fn into_value(self) -> Value {
        match self.kind.as_str() {
            "reasoning" => match self.encrypted {
                Some(encrypted) if self.text.is_empty() => encrypted_reasoning_block(json!(encrypted)),
                _ => reasoning_block(json!(self.text), self.signature),
            },
            "tool_call" => tool_call_block(self.id, self.name, parse_input(&self.input_json)),
            _ => text_block(&self.text),
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
struct Reply {
    blocks: BTreeMap<u64, Block>,
    skipped: HashSet<u64>,
    usage: Map<String, Value>,
    stop_reason: Value,
    bytes: usize,
    truncated: bool,
    unparsed: bool,
}

/// Folds one exchange's common-format live events into finished records.
pub struct Assembler {
    bound: usize,
    conversation: Value,
    reply: Reply,
    seen: bool,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::with_bound(MAX_REQUEST_BODY)
    }
}

impl Assembler {
    pub fn with_bound(bound: usize) -> Self {
        Self {
            bound,
            conversation: Value::Null,
            reply: Reply::default(),
            seen: false,
        }
    }

    /// Returns the records this event completes: one on a reply end, and one more when a new
    /// conversation arrives while a reply is still open.
    pub fn push(&mut self, event: &TapEvent, dropped: u64) -> Vec<Value> {
        let payload = &event.payload;
        let mut records = Vec::new();
        match event.kind.as_str() {
            "conversation" => {
                if self.seen {
                    records.push(self.take_record(dropped, true));
                }
                self.conversation = payload.clone();
            }
            "block_start" => self.start_block(payload),
            "block_delta" => self.add_delta(payload),
            "usage" => self.merge_usage(payload),
            "reply_end" => {
                self.reply.stop_reason = payload["stop_reason"].clone();
                return vec![self.take_record(dropped, false)];
            }
            "unparsed" => self.reply.unparsed = true,
            "block_stop" | "error" => {}
            _ => return records,
        }
        self.seen = true;
        records
    }

    /// Returns the open reply as an incomplete record, if any event was seen since the last record.
    pub fn finish(&mut self, dropped: u64) -> Option<Value> {
        self.seen.then(|| self.take_record(dropped, true))
    }

    fn start_block(&mut self, payload: &Value) {
        let index = payload["index"].as_u64().unwrap_or_default();
        let kind = payload["kind"].as_str().unwrap_or_default();
        if !["text", "reasoning", "tool_call"].contains(&kind) {
            self.reply.skipped.insert(index);
            return;
        }
        self.reply.blocks.insert(
            index,
            Block {
                kind: kind.to_string(),
                id: payload["id"].clone(),
                name: payload["name"].clone(),
                text: String::new(),
                signature: Value::Null,
                encrypted: None,
                input_json: String::new(),
            },
        );
    }

    fn add_delta(&mut self, payload: &Value) {
        let index = payload["index"].as_u64().unwrap_or_default();
        let Some(block) = self.reply.blocks.get_mut(&index) else {
            return;
        };
        for field in ["text", "input_json", "signature", "encrypted"] {
            let Some(piece) = payload[field].as_str() else {
                continue;
            };
            if self.reply.truncated || self.reply.bytes + piece.len() > self.bound {
                self.reply.truncated = true;
                return;
            }
            self.reply.bytes += piece.len();
            match field {
                "text" => block.text.push_str(piece),
                "input_json" => block.input_json.push_str(piece),
                "encrypted" => block.encrypted = Some(piece.to_string()),
                _ => block.signature = json!(piece),
            }
        }
    }

    fn merge_usage(&mut self, payload: &Value) {
        for field in USAGE_FIELDS {
            if !payload[field].is_null() {
                self.reply.usage.insert(field.to_string(), payload[field].clone());
            }
        }
    }

    fn take_record(&mut self, dropped: u64, cut: bool) -> Value {
        let reply = std::mem::take(&mut self.reply);
        let conversation = std::mem::take(&mut self.conversation);
        self.seen = false;
        let incomplete = cut || dropped > 0 || reply.unparsed || reply.truncated;
        let blocks = reply.blocks.into_values().map(Block::into_value).collect();
        let usage: Map<String, Value> = USAGE_FIELDS
            .iter()
            .map(|field| {
                (
                    field.to_string(),
                    reply.usage.get(*field).cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        json!({
            "conversation": conversation,
            "reply": {
                "message": message("assistant", blocks),
                "usage": usage,
                "stop_reason": reply.stop_reason,
            },
            "incomplete": incomplete,
            "dropped": dropped,
        })
    }
}

#[cfg(test)]
mod tests;
