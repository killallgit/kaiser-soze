use super::history::History;
use super::stream::stream_event;
use crate::adapter::{ExchangeParser, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer};
use crate::common::*;
use crate::sse::{SseEvent, SseSplitter};
use serde_json::Value;

pub struct CodexParser {
    pub(super) history: History,
    request: RequestBuffer,
    response_encoded: bool,
    steps: u64,
    sse: SseSplitter,
}

impl CodexParser {
    pub(super) fn new(request_encoded: bool) -> Self {
        Self {
            history: History::default(),
            request: RequestBuffer::new(request_encoded),
            response_encoded: false,
            steps: 0,
            sse: SseSplitter::default(),
        }
    }

    pub(super) fn request_events(&mut self, body: &[u8], start: u64, end: u64) -> Vec<TapEvent> {
        let event = match serde_json::from_slice::<Value>(body) {
            Ok(json) if json["type"].as_str().is_some_and(|kind| kind != "response.create") => return Vec::new(),
            Ok(json) => {
                self.steps += 1;
                let mut event = self.history.join(&json);
                event.payload["step"] = self.steps.into();
                if let Some(turn) = json["client_metadata"]["turn_id"].as_str() {
                    event.payload["turn"] = turn.into();
                }
                event
            }
            Err(_) => {
                self.history.broken = true;
                unparsed(Side::Request)
            }
        };
        vec![event.at(start, end)]
    }

    fn stream_events(&mut self, events: Vec<SseEvent>) -> Vec<TapEvent> {
        events
            .iter()
            .flat_map(|e| {
                self.history.watch(&e.data);
                stream_event(&e.data, e.start, e.end)
            })
            .collect()
    }
}

impl ExchangeParser for CodexParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        match input {
            TapInput::RequestBody(chunk) => {
                self.request.push(&chunk);
                Vec::new()
            }
            TapInput::RequestEnd if self.request.unreadable() => {
                self.history.broken = true;
                vec![unparsed(Side::Request)]
            }
            TapInput::RequestEnd if self.request.bytes().is_empty() => Vec::new(),
            TapInput::RequestEnd => {
                let body = self.request.bytes().to_vec();
                self.request_events(&body, 0, body.len() as u64)
            }
            TapInput::ResponseHead(head) => {
                self.response_encoded = is_encoded(&head.headers);
                Vec::new()
            }
            TapInput::ResponseBody(chunk) if !self.response_encoded => {
                let events = self.sse.push(&chunk);
                self.stream_events(events)
            }
            TapInput::ResponseEnd if self.response_encoded => vec![unparsed(Side::Reply)],
            TapInput::ResponseEnd => {
                let events = self.sse.finish();
                self.stream_events(events)
            }
            TapInput::WsMessage {
                from_client: true,
                text,
                start,
                end,
            } => self.request_events(text.as_bytes(), start, end),
            TapInput::WsMessage { text, start, end, .. } => {
                self.history.watch(&text);
                stream_event(&text, start, end)
            }
            _ => Vec::new(),
        }
    }
}
