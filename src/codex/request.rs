use super::history::append;
use crate::common::*;
use serde_json::{json, Value};

pub(super) fn read_request(json: &Value) -> (Value, Value, Vec<Value>) {
    let mut messages: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    let mut tools = Value::Null;
    for (index, item) in json["input"].as_array().into_iter().flatten().enumerate() {
        match (item["type"].as_str().unwrap_or("message"), item["role"].as_str()) {
            ("additional_tools", _) => tools = item["tools"].clone(),
            ("message", Some("developer" | "system")) => system.extend(system_entries(&item["content"])),
            _ => {
                if let Some((role, blocks)) = input_item(index, item) {
                    append(&mut messages, vec![message(&role, blocks)]);
                }
            }
        }
    }
    let system = match (&json["instructions"], system.is_empty()) {
        (Value::Null, false) => Value::Array(system),
        (instructions, _) => instructions.clone(),
    };
    let tools = if json["tools"].is_null() {
        tools
    } else {
        json["tools"].clone()
    };
    (system, tools, messages)
}

pub(super) fn system_entries(content: &Value) -> Vec<Value> {
    let texts: Vec<&str> = match content {
        Value::String(text) => vec![text],
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect(),
        _ => Vec::new(),
    };
    texts
        .into_iter()
        .map(|text| json!({"type": "text", "text": text}))
        .collect()
}

pub(super) fn input_item(index: usize, item: &Value) -> Option<(String, Vec<Value>)> {
    let location = index.to_string();
    match item["type"].as_str().unwrap_or("message") {
        "message" => {
            let role = if item["role"].as_str()? == "assistant" {
                "assistant"
            } else {
                "user"
            };
            Some((role.to_string(), content_parts(&location, &item["content"])))
        }
        "reasoning" => Some(("assistant".into(), vec![reasoning_of(item)])),
        kind @ ("function_call" | "custom_tool_call") => {
            let input = if kind == "function_call" {
                item["arguments"].as_str().map_or(Value::Null, parse_arguments)
            } else {
                item["input"].clone()
            };
            Some((
                "assistant".into(),
                vec![tool_call_block(item["call_id"].clone(), item["name"].clone(), input)],
            ))
        }
        "function_call_output" | "custom_tool_call_output" => {
            let output = match &item["output"] {
                Value::Array(_) => Value::Array(content_parts(&location, &item["output"])),
                other => other.clone(),
            };
            Some((
                "user".into(),
                vec![tool_result_block(item["call_id"].clone(), output, false)],
            ))
        }
        _ => None,
    }
}

pub(super) fn reasoning_of(item: &Value) -> Value {
    let summary: String = item["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect();
    match item["encrypted_content"].as_str() {
        Some(encrypted) if summary.is_empty() => encrypted_reasoning_block(json!(encrypted)),
        _ => reasoning_block(json!(summary), Value::Null),
    }
}

pub(super) fn parse_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!(arguments))
}

pub(super) fn content_parts(location: &str, content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => vec![text_block(text)],
        Value::Array(parts) => parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| match part["type"].as_str()? {
                "input_text" | "output_text" | "text" => Some(text_block(part["text"].as_str()?)),
                "input_image" => Some(image_of(
                    &format!("{location}:{index}"),
                    part["image_url"].as_str().unwrap_or_default(),
                )),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn image_of(location: &str, url: &str) -> Value {
    let (content_type, size) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .map(|(meta, data)| (meta.split(';').next().unwrap_or_default(), base64_size(data)))
        .unwrap_or_default();
    image_block(location, content_type, size)
}
