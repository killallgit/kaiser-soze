use super::history::append;
use crate::adapter::image::base64_size;
use crate::common::message::{Block, Message, Output};
use serde_json::{json, Value};

pub(super) fn read_request(json: &Value) -> (Value, Value, Vec<Message>) {
    let mut messages: Vec<Message> = Vec::new();
    let mut system = Vec::new();
    let mut tools = Value::Null;
    for (index, item) in json["input"].as_array().into_iter().flatten().enumerate() {
        match (item["type"].as_str().unwrap_or("message"), item["role"].as_str()) {
            ("additional_tools", _) => tools = item["tools"].clone(),
            ("message", Some("developer" | "system")) => system.extend(system_entries(&item["content"])),
            _ => {
                if let Some(message) = input_item(index, item) {
                    append(&mut messages, vec![message]);
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

pub(super) fn input_item(index: usize, item: &Value) -> Option<Message> {
    let location = index.to_string();
    match item["type"].as_str().unwrap_or("message") {
        "message" => {
            let assistant = item["role"].as_str()? == "assistant";
            let blocks = content_parts(&location, &item["content"]);
            Some(if assistant {
                Message::assistant(blocks)
            } else {
                Message::user(blocks)
            })
        }
        "reasoning" => Some(Message::assistant(vec![reasoning_of(item)])),
        kind @ ("function_call" | "custom_tool_call") => {
            let input = if kind == "function_call" {
                item["arguments"].as_str().map_or(Value::Null, parse_arguments)
            } else {
                item["input"].clone()
            };
            Some(Message::assistant(vec![Block::ToolCall {
                id: item["call_id"].clone(),
                input,
                name: item["name"].clone(),
            }]))
        }
        "function_call_output" | "custom_tool_call_output" => {
            let output = match &item["output"] {
                Value::Array(_) => Output::Blocks(content_parts(&location, &item["output"])),
                other => Output::Raw(other.clone()),
            };
            Some(Message::user(vec![Block::ToolResult {
                call_id: item["call_id"].clone(),
                is_error: false,
                output,
            }]))
        }
        _ => None,
    }
}

pub(super) fn reasoning_of(item: &Value) -> Block {
    let summary: String = item["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect();
    match item["encrypted_content"].as_str() {
        Some(encrypted) if summary.is_empty() => Block::EncryptedReasoning {
            encrypted: json!(encrypted),
        },
        _ => Block::reasoning(json!(summary), Value::Null),
    }
}

pub(super) fn parse_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!(arguments))
}

pub(super) fn content_parts(location: &str, content: &Value) -> Vec<Block> {
    match content {
        Value::String(text) => vec![Block::text(text)],
        Value::Array(parts) => parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| match part["type"].as_str()? {
                "input_text" | "output_text" | "text" => Some(Block::text(part["text"].as_str()?)),
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

pub(super) fn image_of(location: &str, url: &str) -> Block {
    let (content_type, size) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .map(|(meta, data)| (meta.split(';').next().unwrap_or_default(), base64_size(data)))
        .unwrap_or_default();
    Block::Image {
        content_type: content_type.to_string(),
        reference: location.to_string(),
        size,
    }
}
