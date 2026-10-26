use crate::adapter::TapEvent;
use crate::message::Message;
use serde_json::{json, Value};

pub struct BlockStart {
    pub index: u64,
    pub kind: &'static str,
    pub id: Value,
    pub name: Value,
}

pub enum Delta {
    Text(String),
    InputJson(String),
    Signature(String),
    Encrypted(String),
}

#[derive(Default)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
}

pub enum Side {
    Request,
    Reply,
}

impl TapEvent {
    pub fn at(mut self, start: u64, end: u64) -> Self {
        self.start = start;
        self.end = end;
        self
    }
}

fn live(kind: &str, payload: Value) -> TapEvent {
    TapEvent {
        kind: kind.to_string(),
        payload,
        start: 0,
        end: 0,
    }
}

pub fn conversation(system: Value, tools: Value, messages: Vec<Message>) -> TapEvent {
    live(
        "conversation",
        json!({"system": system, "tools": tools, "messages": messages}),
    )
}

pub fn block_start(start: BlockStart) -> TapEvent {
    let BlockStart { index, kind, id, name } = start;
    let mut payload = json!({"index": index, "kind": kind});
    if !id.is_null() {
        payload["id"] = id;
    }
    if !name.is_null() {
        payload["name"] = name;
    }
    live("block_start", payload)
}

pub fn block_delta(index: u64, delta: Delta) -> TapEvent {
    let (field, value) = match delta {
        Delta::Text(text) => ("text", text),
        Delta::InputJson(json) => ("input_json", json),
        Delta::Signature(signature) => ("signature", signature),
        Delta::Encrypted(encrypted) => ("encrypted", encrypted),
    };
    live("block_delta", json!({"index": index, field: value}))
}

pub fn block_stop(index: u64) -> TapEvent {
    live("block_stop", json!({"index": index}))
}

pub fn usage(usage: Usage) -> TapEvent {
    live(
        "usage",
        json!({
            "input": usage.input,
            "output": usage.output,
            "cache_read": usage.cache_read,
            "cache_write": usage.cache_write,
            "reasoning": usage.reasoning,
        }),
    )
}

pub fn reply_end(stop_reason: Value) -> TapEvent {
    live("reply_end", json!({"stop_reason": stop_reason}))
}

pub fn reply_end_of_message(stop_reason: Value, message_id: Value) -> TapEvent {
    let mut event = reply_end(stop_reason);
    if !message_id.is_null() {
        event.payload["message_id"] = message_id;
    }
    event
}

pub fn error(error_type: Value) -> TapEvent {
    live("error", json!({"error_type": error_type}))
}

pub fn unsupported(host: &str, path: &str) -> TapEvent {
    live("unsupported", json!({"host": host, "path": path}))
}

pub fn unparsed(side: Side) -> TapEvent {
    let side = match side {
        Side::Request => "request",
        Side::Reply => "reply",
    };
    live("unparsed", json!({"side": side}))
}

pub fn base64_size(data: &str) -> usize {
    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
    (data.len() / 4 * 3).saturating_sub(padding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Block;
    use serde_json::json;

    #[test]
    fn builds_each_live_event_with_its_contract_kind_and_fields() {
        // It names the kinds in contracts/records.md and carries only their fields
        let conversation = conversation(
            json!("sys"),
            json!([{"name": "shell"}]),
            vec![Message::user(vec![Block::text("hi")])],
        );
        let start = block_start(BlockStart {
            index: 1,
            kind: "tool_call",
            id: json!("c1"),
            name: json!("shell"),
        });
        let delta = block_delta(1, Delta::InputJson("{}".into()));
        let stop = block_stop(1);
        let end = reply_end(json!("end_turn"));
        let failed = error(json!("overloaded_error"));
        let unsupported = unsupported("chatgpt.com", "/x");
        let unreadable = unparsed(Side::Reply);

        let kinds: Vec<_> = [
            &conversation,
            &start,
            &delta,
            &stop,
            &end,
            &failed,
            &unsupported,
            &unreadable,
        ]
        .iter()
        .map(|event| event.kind.as_str())
        .collect();

        assert_eq!(
            kinds,
            [
                "conversation",
                "block_start",
                "block_delta",
                "block_stop",
                "reply_end",
                "error",
                "unsupported",
                "unparsed"
            ]
        );
        assert_eq!(conversation.payload["messages"][0]["role"], "user");
        assert_eq!(
            start.payload,
            json!({"index": 1, "kind": "tool_call", "id": "c1", "name": "shell"})
        );
        assert_eq!(delta.payload, json!({"index": 1, "input_json": "{}"}));
        assert_eq!(stop.payload, json!({"index": 1}));
        assert_eq!(end.payload, json!({"stop_reason": "end_turn"}));
        assert_eq!(failed.payload, json!({"error_type": "overloaded_error"}));
        assert_eq!(unsupported.payload, json!({"host": "chatgpt.com", "path": "/x"}));
        assert_eq!(unreadable.payload, json!({"side": "reply"}));
    }

    #[test]
    fn usage_carries_counts_or_null_for_each_field() {
        // It keeps all five fields and leaves a missing count null
        let event = usage(Usage {
            input: Some(3),
            output: Some(1),
            ..Usage::default()
        });

        assert_eq!(event.kind, "usage");
        assert_eq!(
            event.payload,
            json!({"input": 3, "output": 1, "cache_read": null, "cache_write": null, "reasoning": null})
        );
    }

    #[test]
    fn at_sets_the_byte_span_of_an_event() {
        // It moves the event to the given start and end offsets
        let event = block_stop(0).at(4, 9);

        assert_eq!((event.start, event.end), (4, 9));
    }
}
