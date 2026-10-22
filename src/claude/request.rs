use crate::adapter::TapEvent;
use crate::common::*;
use serde_json::Value;

pub(super) fn conversation_event(json: &Value) -> TapEvent {
    let messages = json["messages"].as_array().map_or_else(Vec::new, |messages| {
        messages
            .iter()
            .enumerate()
            .map(|(index, m)| {
                message(
                    m["role"].as_str().unwrap_or("user"),
                    content_blocks(&index.to_string(), &m["content"]),
                )
            })
            .collect()
    });
    conversation(json["system"].clone(), json["tools"].clone(), messages)
}

pub(super) fn content_blocks(location: &str, content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => vec![text_block(text)],
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| block(&format!("{location}:{index}"), item))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn block(location: &str, item: &Value) -> Option<Value> {
    Some(match item["type"].as_str()? {
        "text" => text_block(item["text"].as_str()?),
        "thinking" => reasoning_block(item["thinking"].clone(), item["signature"].clone()),
        "redacted_thinking" => encrypted_reasoning_block(item["data"].clone()),
        "tool_use" => tool_call_block(item["id"].clone(), item["name"].clone(), item["input"].clone()),
        "tool_result" => tool_result_block(
            item["tool_use_id"].clone(),
            result_output(location, &item["content"]),
            item["is_error"].as_bool().unwrap_or(false),
        ),
        "image" => image_block(
            location,
            item["source"]["media_type"].as_str().unwrap_or_default(),
            base64_size(item["source"]["data"].as_str().unwrap_or_default()),
        ),
        _ => return None,
    })
}

pub(super) fn result_output(location: &str, content: &Value) -> Value {
    match content {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| block(&format!("{location}:{index}"), item))
                .collect(),
        ),
        other => other.clone(),
    }
}
