use crate::adapter::{header, Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer};
use crate::common::*;
use crate::graph::Identity;
use crate::sse::{SseEvent, SseSplitter};
use serde_json::Value;

pub struct ClaudeAdapter {
    hosts: Vec<String>,
}

impl ClaudeAdapter {
    pub fn new(hosts: Vec<String>) -> Self {
        Self { hosts }
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new(vec!["api.anthropic.com".into()])
    }
}

impl Adapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn hosts(&self) -> &[String] {
        &self.hosts
    }

    fn claim(&self, head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        let path = head.path.split('?').next().unwrap_or_default();
        (path == MESSAGES_PATH).then(|| Box::new(ClaudeParser::new(head)) as Box<dyn ExchangeParser>)
    }

    /// The main agent has no agent id and is the session itself; a subagent's parent is the agent that
    /// launched it, else the main agent.
    fn identity(&self, head: &RequestHead) -> Identity {
        let owned = |name| header(head, name).map(str::to_string);
        let session = owned("x-claude-code-session-id");
        let thread = owned("x-claude-code-agent-id").or_else(|| session.clone());
        let subagent = header(head, "x-claude-code-request-class") == Some("subagent");
        let parent_thread = owned("x-claude-code-parent-agent-id").or(session.clone().filter(|_| subagent));
        Identity {
            session,
            thread,
            parent_thread,
            turn: owned("x-claude-code-prompt-id"),
        }
    }
}

const MESSAGES_PATH: &str = "/v1/messages";

pub struct ClaudeParser {
    request: RequestBuffer,
    response_sse: bool,
    response_encoded: bool,
    stop_reason: Value,
    sse: SseSplitter,
}

impl ClaudeParser {
    fn new(head: &RequestHead) -> Self {
        Self {
            request: RequestBuffer::new(is_encoded(&head.headers)),
            response_sse: false,
            response_encoded: false,
            stop_reason: Value::Null,
            sse: SseSplitter::default(),
        }
    }

    fn request_events(&self) -> Vec<TapEvent> {
        let body = self.request.bytes();
        let parsed = serde_json::from_slice::<Value>(body)
            .ok()
            .filter(|_| !self.request.unreadable());
        let event = match parsed {
            Some(json) => conversation_event(&json),
            None => unparsed(Side::Request),
        };
        vec![event.at(0, body.len() as u64)]
    }

    fn stream_events(&mut self, events: Vec<SseEvent>) -> Vec<TapEvent> {
        events
            .iter()
            .flat_map(|e| {
                self.stream_event(&e.data)
                    .into_iter()
                    .map(|event| event.at(e.start, e.end))
            })
            .collect()
    }

    fn stream_event(&mut self, data: &str) -> Vec<TapEvent> {
        let Ok(json) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        let index = json["index"].as_u64().unwrap_or_default();
        match json["type"].as_str().unwrap_or_default() {
            "message_start" => vec![usage_event(&json["message"]["usage"])],
            "content_block_start" => vec![block_start(block_start_of(index, &json["content_block"]))],
            "content_block_delta" => block_delta_of(index, &json["delta"]).into_iter().collect(),
            "content_block_stop" => vec![block_stop(index)],
            "message_delta" => {
                self.stop_reason = json["delta"]["stop_reason"].clone();
                vec![usage_event(&json["usage"])]
            }
            "message_stop" => vec![reply_end(self.stop_reason.clone())],
            "error" => vec![error(json["error"]["type"].clone())],
            _ => Vec::new(),
        }
    }
}

fn conversation_event(json: &Value) -> TapEvent {
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

fn content_blocks(location: &str, content: &Value) -> Vec<Value> {
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

fn block(location: &str, item: &Value) -> Option<Value> {
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

fn result_output(location: &str, content: &Value) -> Value {
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

fn usage_event(usage_json: &Value) -> TapEvent {
    let count = |field: &str| usage_json[field].as_u64();
    usage(Usage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_input_tokens"),
        cache_write: count("cache_creation_input_tokens"),
        reasoning: None,
    })
}

fn block_start_of(index: u64, block: &Value) -> BlockStart {
    let (kind, id, name) = match block["type"].as_str().unwrap_or_default() {
        "text" => ("text", Value::Null, Value::Null),
        "thinking" | "redacted_thinking" => ("reasoning", Value::Null, Value::Null),
        "tool_use" => ("tool_call", block["id"].clone(), block["name"].clone()),
        _ => ("other", Value::Null, Value::Null),
    };
    BlockStart { index, kind, id, name }
}

fn block_delta_of(index: u64, delta: &Value) -> Option<TapEvent> {
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

impl ExchangeParser for ClaudeParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        match input {
            TapInput::RequestBody(chunk) => {
                self.request.push(&chunk);
                Vec::new()
            }
            TapInput::RequestEnd => self.request_events(),
            TapInput::ResponseHead(head) => {
                self.response_encoded = is_encoded(&head.headers);
                self.response_sse = head
                    .headers
                    .iter()
                    .any(|(name, value)| name == "content-type" && value.contains("event-stream"));
                Vec::new()
            }
            TapInput::ResponseBody(chunk) if self.response_sse && !self.response_encoded => {
                let events = self.sse.push(&chunk);
                self.stream_events(events)
            }
            TapInput::ResponseEnd if self.response_sse && !self.response_encoded => {
                let events = self.sse.finish();
                self.stream_events(events)
            }
            TapInput::ResponseEnd => vec![unparsed(Side::Reply)],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
