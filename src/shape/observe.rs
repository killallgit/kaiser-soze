use super::{shape, ShapeLog};
use crate::adapter::body::{is_encoded, RequestBuffer};
use crate::adapter::sse::SseSplitter;
use crate::adapter::{ExchangeParser, RequestHead, ResponseHead, TapEvent, TapInput};
use serde_json::{json, Value};
use std::sync::Arc;

/// Watches one exchange and logs its shapes, then hands every input on to the real parser.
pub struct ShapedParser {
    log: Arc<ShapeLog>,
    inner: Box<dyn ExchangeParser>,
    exchange: u64,
    host: String,
    path: String,
    request: RequestBuffer,
    response: Vec<u8>,
    splitter: SseSplitter,
    transport: &'static str,
}

impl ShapedParser {
    pub fn new(log: Arc<ShapeLog>, inner: Box<dyn ExchangeParser>, exchange: u64, head: &RequestHead) -> Self {
        let parser = Self {
            log,
            inner,
            exchange,
            host: head.host.clone(),
            path: head.path.split('?').next().unwrap_or_default().to_string(),
            request: RequestBuffer::new(is_encoded(&head.headers)),
            response: Vec::new(),
            splitter: SseSplitter::default(),
            transport: "json",
        };
        parser.note(
            "request_head",
            "http",
            Value::Null,
            json!({"method": head.method, "headers": header_names(&head.headers)}),
        );
        parser
    }

    fn note(&self, direction: &str, transport: &str, body: Value, extra: Value) {
        let kind = body.get("type").and_then(Value::as_str).map(str::to_string);
        let mut line = json!({
            "host": self.host,
            "path": self.path,
            "direction": direction,
            "transport": transport,
            "kind": kind,
            "shape": if body.is_null() { Value::Null } else { shape(&body) },
        });
        if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
            line.extend(extra.clone());
        }
        self.log.record(self.exchange, line);
    }

    fn note_text(&self, direction: &str, transport: &str, text: &str) {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            self.note(direction, transport, body, json!({}));
        }
    }

    fn observe(&mut self, input: &TapInput) {
        match input {
            TapInput::RequestBody(chunk) => self.request.push(chunk),
            TapInput::RequestEnd if !self.request.unreadable() => {
                if let Ok(body) = serde_json::from_slice(self.request.bytes()) {
                    self.note("request", "json", body, json!({}));
                }
            }
            TapInput::ResponseHead(head) => self.observe_response_head(head),
            TapInput::ResponseBody(chunk) if self.transport == "sse" => {
                for event in self.splitter.push(chunk) {
                    self.note_text("response", "sse", &event.data);
                }
            }
            TapInput::ResponseBody(chunk) => self.response.extend_from_slice(chunk),
            TapInput::ResponseEnd if self.transport == "sse" => {
                for event in self.splitter.finish() {
                    self.note_text("response", "sse", &event.data);
                }
            }
            TapInput::ResponseEnd => {
                if let Ok(body) = serde_json::from_slice(&self.response) {
                    self.note("response", "json", body, json!({}));
                }
            }
            TapInput::WsMessage { from_client, text, .. } => {
                let direction = if *from_client { "request" } else { "response" };
                self.note_text(direction, "websocket", text);
            }
            _ => {}
        }
    }

    fn observe_response_head(&mut self, head: &ResponseHead) {
        let streaming = head
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.starts_with("text/event-stream"));
        self.transport = if streaming {
            "sse"
        } else if head.status == 101 {
            "websocket"
        } else {
            "json"
        };
        self.note(
            "response_head",
            self.transport,
            Value::Null,
            json!({"status": head.status, "headers": header_names(&head.headers)}),
        );
    }
}

impl ExchangeParser for ShapedParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        self.observe(&input);
        self.inner.feed(input)
    }
}

fn header_names(headers: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = headers.iter().map(|(name, _)| name.to_lowercase()).collect();
    names.sort();
    names.dedup();
    names
}
