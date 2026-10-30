use crate::adapter::event::{block_delta, block_start, block_stop, error, reply_end_of_message, usage};
use crate::adapter::TapEvent;
use crate::common::*;
use serde_json::Value;

pub(super) fn stream_event(json: &Value, start: u64, end: u64, created_id: &mut Value) -> Vec<TapEvent> {
    let index = json["output_index"].as_u64().unwrap_or_default();
    let delta = || json["delta"].as_str().map(String::from);
    let events = match json["type"].as_str().unwrap_or_default() {
        "response.output_item.added" => vec![block_start(block_start_of(index, &json["item"]))],
        "response.output_text.delta" | "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            delta()
                .map(|text| block_delta(index, Delta::Text(text)))
                .into_iter()
                .collect()
        }
        "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => delta()
            .map(|json| block_delta(index, Delta::InputJson(json)))
            .into_iter()
            .collect(),
        "response.output_item.done" => {
            let encrypted = json["item"]["encrypted_content"].as_str();
            encrypted
                .map(|encrypted| block_delta(index, Delta::Encrypted(encrypted.into())))
                .into_iter()
                .chain([block_stop(index)])
                .collect()
        }
        "response.created" => {
            *created_id = json["response"]["id"].clone();
            Vec::new()
        }
        "response.completed" | "response.incomplete" => {
            let response = &json["response"];
            let remembered = std::mem::take(created_id);
            let id = if response["id"].is_null() {
                remembered
            } else {
                response["id"].clone()
            };
            vec![
                usage_event(&response["usage"]),
                reply_end_of_message(response["status"].clone(), id),
            ]
        }
        "response.failed" => {
            *created_id = Value::Null;
            vec![error(json["response"]["error"]["code"].clone())]
        }
        "error" => {
            *created_id = Value::Null;
            vec![error(json["error"]["code"].clone())]
        }
        _ => Vec::new(),
    };
    events.into_iter().map(|event| event.at(start, end)).collect()
}

pub(super) fn block_start_of(index: u64, item: &Value) -> BlockStart {
    let (kind, id, name) = match item["type"].as_str().unwrap_or_default() {
        "message" => (BlockKind::Text, Value::Null, Value::Null),
        "reasoning" => (BlockKind::Reasoning, Value::Null, Value::Null),
        "function_call" | "custom_tool_call" => (BlockKind::ToolCall, item["call_id"].clone(), item["name"].clone()),
        _ => (BlockKind::Other, Value::Null, Value::Null),
    };
    BlockStart { id, index, kind, name }
}

pub(super) fn usage_event(usage_json: &Value) -> TapEvent {
    usage(Usage {
        input: usage_json["input_tokens"].as_u64(),
        output: usage_json["output_tokens"].as_u64(),
        cache_read: usage_json["input_tokens_details"]["cached_tokens"].as_u64(),
        cache_write: None,
        reasoning: usage_json["output_tokens_details"]["reasoning_tokens"].as_u64(),
    })
}
