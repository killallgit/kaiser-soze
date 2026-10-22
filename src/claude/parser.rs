use super::request::conversation_event;
use super::stream::{block_delta_of, block_start_of, usage_event};
use crate::adapter::{ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer};
use crate::common::*;
use crate::sse::{SseEvent, SseSplitter};
use serde_json::Value;

pub struct ClaudeParser {
    request: RequestBuffer,
    response_sse: bool,
    response_encoded: bool,
    message_id: Value,
    stop_reason: Value,
    sse: SseSplitter,
}

impl ClaudeParser {
    pub(super) fn new(head: &RequestHead) -> Self {
        Self {
            request: RequestBuffer::new(is_encoded(&head.headers)),
            response_sse: false,
            response_encoded: false,
            message_id: Value::Null,
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
            "message_start" => {
                self.message_id = json["message"]["id"].clone();
                vec![usage_event(&json["message"]["usage"])]
            }
            "content_block_start" => vec![block_start(block_start_of(index, &json["content_block"]))],
            "content_block_delta" => block_delta_of(index, &json["delta"]).into_iter().collect(),
            "content_block_stop" => vec![block_stop(index)],
            "message_delta" => {
                self.stop_reason = json["delta"]["stop_reason"].clone();
                vec![usage_event(&json["usage"])]
            }
            "message_stop" => vec![reply_end_of_message(self.stop_reason.clone(), self.message_id.clone())],
            "error" => vec![error(json["error"]["type"].clone())],
            _ => Vec::new(),
        }
    }
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
