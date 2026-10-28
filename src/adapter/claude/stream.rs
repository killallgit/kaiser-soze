use crate::adapter::event::{block_delta, usage};
use crate::adapter::TapEvent;
use crate::common::*;
use serde_json::Value;

pub(super) fn usage_event(usage_json: &Value) -> TapEvent {
    let count = |field: &str| usage_json[field].as_u64();
    usage(Usage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_input_tokens"),
        cache_write: count("cache_creation_input_tokens"),
        reasoning: None,
    })
}

pub(super) fn block_start_of(index: u64, block: &Value) -> BlockStart {
    let (kind, id, name) = match block["type"].as_str().unwrap_or_default() {
        "text" => (BlockKind::Text, Value::Null, Value::Null),
        "thinking" | "redacted_thinking" => (BlockKind::Reasoning, Value::Null, Value::Null),
        "tool_use" => (BlockKind::ToolCall, block["id"].clone(), block["name"].clone()),
        _ => (BlockKind::Other, Value::Null, Value::Null),
    };
    BlockStart { id, index, kind, name }
}

pub(super) fn block_delta_of(index: u64, delta: &Value) -> Option<TapEvent> {
    let text = |field: &str| delta[field].as_str().map(String::from);
    let piece = match delta["type"].as_str()? {
        "text_delta" => Delta::Text(text("text")?),
        "thinking_delta" => Delta::Text(text("thinking")?),
        "signature_delta" => Delta::Signature(text("signature")?),
        "input_json_delta" => Delta::InputJson(text("partial_json")?),
        _ => return None,
    };
    Some(block_delta(index, piece))
}
