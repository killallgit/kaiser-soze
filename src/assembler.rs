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
mod tests {
    use super::*;
    use crate::common::*;
    use serde_json::json;

    fn conversation_event() -> TapEvent {
        conversation(json!("sys"), json!([]), vec![message("user", vec![text_block("hi")])])
    }

    fn text_reply(text: &str) -> Vec<TapEvent> {
        vec![
            block_start(BlockStart {
                index: 0,
                kind: "text",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Text(text.into())),
            block_stop(0),
            usage(Usage {
                input: Some(3),
                output: Some(1),
                ..Usage::default()
            }),
            reply_end(json!("end_turn")),
        ]
    }

    fn feed(assembler: &mut Assembler, events: Vec<TapEvent>, dropped: u64) -> Vec<Value> {
        events.iter().flat_map(|event| assembler.push(event, dropped)).collect()
    }

    #[test]
    fn a_complete_exchange_yields_one_complete_record() {
        // It folds the events into conversation, reply, usage and stop reason
        let mut assembler = Assembler::default();
        let mut events = vec![conversation_event()];
        events.extend(text_reply("ok"));

        let records = feed(&mut assembler, events, 0);

        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record["incomplete"], false);
        assert_eq!(record["dropped"], 0);
        assert_eq!(record["conversation"]["system"], "sys");
        assert_eq!(record["reply"]["message"], message("assistant", vec![text_block("ok")]));
        assert_eq!(record["reply"]["usage"]["input"], 3);
        assert_eq!(record["reply"]["usage"]["output"], 1);
        assert_eq!(record["reply"]["stop_reason"], "end_turn");
        assert!(assembler.finish(0).is_none());
    }

    #[test]
    fn dropped_events_mark_the_record_incomplete_and_carry_the_count() {
        // It reports the count it was given
        let mut assembler = Assembler::default();
        let mut events = vec![conversation_event()];
        events.extend(text_reply("ok"));

        let records = feed(&mut assembler, events, 4);

        assert_eq!(records[0]["incomplete"], true);
        assert_eq!(records[0]["dropped"], 4);
    }

    #[test]
    fn finishing_with_an_open_reply_writes_one_incomplete_record() {
        // It keeps what was assembled when the stream is cut off
        let mut assembler = Assembler::default();
        feed(&mut assembler, vec![conversation_event()], 0);
        feed(&mut assembler, text_reply("ok").into_iter().take(2).collect(), 0);

        let record = assembler.finish(0).unwrap();

        assert_eq!(record["incomplete"], true);
        assert_eq!(record["reply"]["message"], message("assistant", vec![text_block("ok")]));
        assert!(assembler.finish(0).is_none());
    }

    #[test]
    fn an_unparsed_request_or_reply_marks_the_record_incomplete() {
        // It treats both sides the same way
        for side in [Side::Request, Side::Reply] {
            let mut assembler = Assembler::default();
            let mut events = vec![unparsed(side)];
            events.extend(text_reply("ok"));

            let records = feed(&mut assembler, events, 0);

            assert_eq!(records[0]["incomplete"], true);
        }
    }

    #[test]
    fn two_replies_on_one_exchange_yield_two_records() {
        // It resets between replies, as a WebSocket does
        let mut assembler = Assembler::default();
        let mut events = vec![conversation_event()];
        events.extend(text_reply("one"));
        events.push(conversation_event());
        events.extend(text_reply("two"));

        let records = feed(&mut assembler, events, 0);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "one");
        assert_eq!(records[1]["reply"]["message"]["blocks"][0]["text"], "two");
    }

    #[test]
    fn an_item_outside_the_format_is_skipped_and_later_blocks_keep_their_order() {
        // It leaves the item out without marking the record incomplete
        let mut assembler = Assembler::default();
        let events = vec![
            conversation_event(),
            block_start(BlockStart {
                index: 0,
                kind: "web_search",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Text("skipped".into())),
            block_start(BlockStart {
                index: 1,
                kind: "text",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(1, Delta::Text("kept".into())),
            reply_end(json!("end_turn")),
        ];

        let records = feed(&mut assembler, events, 0);

        assert_eq!(records[0]["reply"]["message"]["blocks"], json!([text_block("kept")]));
        assert_eq!(records[0]["incomplete"], false);
    }

    #[test]
    fn a_tool_call_joins_its_json_fragments_into_the_input() {
        // It parses the joined fragments
        let mut assembler = Assembler::default();
        let events = vec![
            conversation_event(),
            block_start(BlockStart {
                index: 0,
                kind: "tool_call",
                id: json!("c1"),
                name: json!("shell"),
            }),
            block_delta(0, Delta::InputJson("{\"a\"".into())),
            block_delta(0, Delta::InputJson(":1}".into())),
            reply_end(json!("tool_use")),
        ];

        let records = feed(&mut assembler, events, 0);

        assert_eq!(
            records[0]["reply"]["message"]["blocks"],
            json!([tool_call_block(json!("c1"), json!("shell"), json!({"a": 1}))])
        );
    }

    #[test]
    fn a_reply_past_the_bound_stops_assembling_and_is_incomplete() {
        // It keeps the part assembled before the bound
        let mut assembler = Assembler::with_bound(5);
        let events = vec![
            conversation_event(),
            block_start(BlockStart {
                index: 0,
                kind: "text",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Text("abc".into())),
            block_delta(0, Delta::Text("defgh".into())),
            reply_end(json!("end_turn")),
        ];

        let records = feed(&mut assembler, events, 0);

        assert_eq!(records[0]["reply"]["message"]["blocks"], json!([text_block("abc")]));
        assert_eq!(records[0]["incomplete"], true);
    }

    #[test]
    fn events_outside_the_common_kinds_write_no_record() {
        // It ignores them, so an unconverted adapter is unaffected
        let mut assembler = Assembler::default();
        let old = TapEvent {
            kind: "message_stop".into(),
            payload: json!({}),
            start: 0,
            end: 0,
        };

        let records = feed(&mut assembler, vec![old], 0);

        assert!(records.is_empty());
        assert!(assembler.finish(0).is_none());
    }

    #[test]
    fn encrypted_reasoning_without_text_becomes_an_encrypted_block() {
        // It keeps the encrypted text when there is no readable text
        let mut assembler = Assembler::default();
        let events = vec![
            block_start(BlockStart {
                index: 0,
                kind: "reasoning",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Encrypted("enc".into())),
            reply_end(json!("completed")),
        ];

        let records = feed(&mut assembler, events, 0);

        assert_eq!(
            records[0]["reply"]["message"]["blocks"],
            json!([encrypted_reasoning_block(json!("enc"))])
        );
    }

    #[test]
    fn usage_fields_merge_across_events() {
        // A later count replaces an earlier one and a missing count keeps the earlier one
        let mut assembler = Assembler::default();
        let events = vec![
            conversation_event(),
            usage(Usage {
                input: Some(9),
                output: Some(1),
                ..Usage::default()
            }),
            usage(Usage {
                output: Some(4),
                ..Usage::default()
            }),
            reply_end(json!("end_turn")),
        ];

        let records = feed(&mut assembler, events, 0);

        assert_eq!(records[0]["reply"]["usage"]["input"], 9);
        assert_eq!(records[0]["reply"]["usage"]["output"], 4);
    }
}
