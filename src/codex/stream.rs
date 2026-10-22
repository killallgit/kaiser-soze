use crate::adapter::TapEvent;
use crate::common::*;
use serde_json::Value;

pub(super) fn stream_event(data: &str, start: u64, end: u64) -> Vec<TapEvent> {
    let Ok(json) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
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
        "response.completed" | "response.incomplete" => {
            let response = &json["response"];
            vec![usage_event(&response["usage"]), reply_end(response["status"].clone())]
        }
        "response.failed" => vec![error(json["response"]["error"]["code"].clone())],
        "error" => vec![error(json["error"]["code"].clone())],
        _ => Vec::new(),
    };
    events.into_iter().map(|event| event.at(start, end)).collect()
}

pub(super) fn block_start_of(index: u64, item: &Value) -> BlockStart {
    let (kind, id, name) = match item["type"].as_str().unwrap_or_default() {
        "message" => ("text", Value::Null, Value::Null),
        "reasoning" => ("reasoning", Value::Null, Value::Null),
        "function_call" | "custom_tool_call" => ("tool_call", item["call_id"].clone(), item["name"].clone()),
        _ => ("other", Value::Null, Value::Null),
    };
    BlockStart { index, kind, id, name }
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
