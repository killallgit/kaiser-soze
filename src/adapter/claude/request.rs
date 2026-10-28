use crate::adapter::image::base64_size;
use crate::adapter::TapEvent;
use crate::common::event::{Conversation, Event};
use crate::common::message::{Block, Message, Output, Role};
use serde_json::Value;

pub(super) fn conversation_event(json: &Value) -> TapEvent {
    let messages = json["messages"].as_array().map_or_else(Vec::new, |messages| {
        messages
            .iter()
            .enumerate()
            .map(|(index, m)| Message {
                blocks: content_blocks(&index.to_string(), &m["content"]),
                role: Role::from(m["role"].as_str().unwrap_or("user")),
            })
            .collect()
    });
    let conversation = Conversation {
        previous_message_id: json["diagnostics"]["previous_message_id"].as_str().map(String::from),
        ..Conversation::new(json["system"].clone(), json["tools"].clone(), messages)
    };
    TapEvent::from(Event::Conversation(conversation))
}

pub(super) fn content_blocks(location: &str, content: &Value) -> Vec<Block> {
    match content {
        Value::String(text) => vec![Block::text(text)],
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| block(&format!("{location}:{index}"), item))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn block(location: &str, item: &Value) -> Option<Block> {
    Some(match item["type"].as_str()? {
        "text" => Block::text(item["text"].as_str()?),
        "thinking" => Block::reasoning(item["thinking"].clone(), item["signature"].clone()),
        "redacted_thinking" => Block::EncryptedReasoning {
            encrypted: item["data"].clone(),
        },
        "tool_use" => Block::ToolCall {
            id: item["id"].clone(),
            input: item["input"].clone(),
            name: item["name"].clone(),
        },
        "tool_result" => Block::ToolResult {
            call_id: item["tool_use_id"].clone(),
            is_error: item["is_error"].as_bool().unwrap_or(false),
            output: result_output(location, &item["content"]),
        },
        "image" => Block::Image {
            content_type: item["source"]["media_type"].as_str().unwrap_or_default().to_string(),
            reference: location.to_string(),
            size: base64_size(item["source"]["data"].as_str().unwrap_or_default()),
        },
        _ => return None,
    })
}

pub(super) fn result_output(location: &str, content: &Value) -> Output {
    match content {
        Value::Array(items) => Output::Blocks(
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| block(&format!("{location}:{index}"), item))
                .collect(),
        ),
        other => Output::Raw(other.clone()),
    }
}
