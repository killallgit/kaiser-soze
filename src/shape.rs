use crate::adapter::{ExchangeParser, RequestHead, ResponseHead, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer};
use crate::sink::Sink;
use crate::sse::SseSplitter;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

const MAX_DEPTH: usize = 12;
const MAX_VARIANTS: usize = 8;
const MAX_SHAPES: usize = 10_000;
/// Fields whose string value is a schema tag, not content, so the value is kept.
const TAGS: [&str; 3] = ["type", "role", "object"];
/// Fields that hold a user-defined schema or free-form data, so only a marker is kept.
const OPAQUE: [&str; 3] = ["input_schema", "parameters", "metadata"];

/// Reduces a JSON value to its shape: keys and types, never content.
pub fn shape(value: &Value) -> Value {
    walk(value, None, 0)
}

fn walk(value: &Value, key: Option<&str>, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return json!("deep");
    }
    match value {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("boolean"),
        Value::Number(_) => json!("number"),
        Value::String(text) if key.is_some_and(|k| TAGS.contains(&k)) && text.len() <= 64 => {
            json!(format!("string={text}"))
        }
        Value::String(_) => json!("string"),
        Value::Array(items) => {
            let mut variants: Vec<Value> = Vec::new();
            for item in items {
                let item_shape = walk(item, None, depth + 1);
                if variants.len() < MAX_VARIANTS && !variants.contains(&item_shape) {
                    variants.push(item_shape);
                }
            }
            Value::Array(variants)
        }
        Value::Object(_) if key.is_some_and(|k| OPAQUE.contains(&k) || k == "input") => json!("opaque"),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(name, child)| (name.clone(), walk(child, Some(name), depth + 1)))
                .collect::<Map<_, _>>(),
        ),
    }
}

/// Writes each distinct shape once per run to its own file.
pub struct ShapeLog {
    sink: Sink,
    seen: Mutex<HashSet<String>>,
}

impl ShapeLog {
    pub fn new(sink: Sink) -> Arc<Self> {
        Arc::new(Self {
            sink,
            seen: Mutex::default(),
        })
    }

    fn record(&self, exchange: u64, line: Value) {
        let key = line.to_string();
        let Ok(mut seen) = self.seen.lock() else { return };
        if seen.len() >= MAX_SHAPES || !seen.insert(key) {
            return;
        }
        drop(seen);
        let mut line = line;
        line["exchange"] = json!(exchange);
        self.sink.write(line);
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    struct Silent;

    impl ExchangeParser for Silent {
        fn feed(&mut self, _input: TapInput) -> Vec<TapEvent> {
            Vec::new()
        }
    }

    fn head() -> RequestHead {
        RequestHead {
            method: "POST".into(),
            host: "api.example.com".into(),
            path: "/v1/messages?beta=true".into(),
            headers: vec![("X-Version".into(), "secret-value".into())],
        }
    }

    fn parser() -> (ShapedParser, tokio::sync::mpsc::Receiver<Value>) {
        let (sink, rx) = Sink::detached(64);
        (ShapedParser::new(ShapeLog::new(sink), Box::new(Silent), 7, &head()), rx)
    }

    fn lines(mut rx: tokio::sync::mpsc::Receiver<Value>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(line);
        }
        out
    }

    #[test]
    fn keeps_keys_and_types_and_drops_values() {
        let body = json!({"model": "m", "n": 3, "ok": true, "none": null, "items": [{"a": "x"}, {"a": "y"}]});

        let got = shape(&body);

        assert_eq!(
            got,
            json!({"model": "string", "n": "number", "ok": "boolean", "none": "null", "items": [{"a": "string"}]})
        );
    }

    #[test]
    fn keeps_schema_tags_and_distinct_variants_of_an_array() {
        let body = json!({"content": [{"type": "text", "text": "hi"}, {"type": "tool_use", "input": {"k": 1}}], "role": "user"});

        let got = shape(&body);

        assert_eq!(
            got,
            json!({
                "role": "string=user",
                "content": [{"type": "string=text", "text": "string"}, {"type": "string=tool_use", "input": "opaque"}]
            })
        );
    }

    #[test]
    fn collapses_user_defined_schemas_but_keeps_an_input_array() {
        let body = json!({"tools": [{"input_schema": {"properties": {"p": {}}}}], "input": [{"role": "user"}]});

        let got = shape(&body);

        assert_eq!(
            got,
            json!({"tools": [{"input_schema": "opaque"}], "input": [{"role": "string=user"}]})
        );
    }

    #[test]
    fn stops_at_a_depth_limit() {
        let mut body = json!("leaf");
        for _ in 0..30 {
            body = json!({"a": body});
        }

        assert!(shape(&body).to_string().contains("deep"));
    }

    #[test]
    fn logs_header_names_without_values_and_the_query_free_path() {
        let (_parser, rx) = parser();

        let got = lines(rx);

        assert_eq!(got[0]["headers"], json!(["x-version"]));
        assert_eq!(got[0]["path"], json!("/v1/messages"));
        assert!(!got[0].to_string().contains("secret-value"));
    }

    #[test]
    fn logs_a_request_body_shape_once_however_often_it_repeats() {
        let (mut parser, rx) = parser();

        for _ in 0..3 {
            parser.feed(TapInput::RequestBody(Bytes::from_static(
                br#"{"model":"m","stream":true}"#,
            )));
            parser.feed(TapInput::RequestEnd);
            parser.request = RequestBuffer::new(false);
        }

        let requests: Vec<Value> = lines(rx).into_iter().filter(|l| l["direction"] == "request").collect();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["shape"], json!({"model": "string", "stream": "boolean"}));
        assert_eq!(requests[0]["exchange"], json!(7));
    }

    #[test]
    fn logs_each_server_sent_event_with_its_type() {
        let (mut parser, rx) = parser();
        parser.feed(TapInput::ResponseHead(ResponseHead {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
        }));

        parser.feed(TapInput::ResponseBody(Bytes::from_static(
            b"event: x\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"i\"}}\n\ndata: [DONE]\n\n",
        )));
        parser.feed(TapInput::ResponseEnd);

        let frames: Vec<Value> = lines(rx)
            .into_iter()
            .filter(|l| l["transport"] == "sse" && l["direction"] == "response")
            .collect();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["kind"], json!("message_start"));
        assert_eq!(frames[0]["shape"]["message"], json!({"id": "string"}));
    }

    #[test]
    fn logs_websocket_messages_by_direction() {
        let (mut parser, rx) = parser();
        let message = |from_client, text: &str| TapInput::WsMessage {
            from_client,
            text: text.into(),
            start: 0,
            end: 0,
        };

        parser.feed(message(true, r#"{"type":"response.create","input":[]}"#));
        parser.feed(message(false, r#"{"type":"response.completed"}"#));
        parser.feed(message(false, "not json"));

        let got: Vec<(String, String)> = lines(rx)
            .into_iter()
            .filter(|l| l["transport"] == "websocket")
            .map(|l| {
                (
                    l["direction"].as_str().unwrap().into(),
                    l["kind"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("request".to_string(), "response.create".to_string()),
                ("response".to_string(), "response.completed".to_string())
            ]
        );
    }

    #[test]
    fn hands_every_input_on_to_the_real_parser() {
        struct Counts(Arc<Mutex<usize>>);
        impl ExchangeParser for Counts {
            fn feed(&mut self, _input: TapInput) -> Vec<TapEvent> {
                *self.0.lock().unwrap() += 1;
                Vec::new()
            }
        }
        let count = Arc::new(Mutex::new(0));
        let (sink, _rx) = Sink::detached(64);
        let mut parser = ShapedParser::new(ShapeLog::new(sink), Box::new(Counts(count.clone())), 1, &head());

        parser.feed(TapInput::RequestBody(Bytes::from_static(b"not json")));
        parser.feed(TapInput::RequestEnd);
        parser.feed(TapInput::ResponseEnd);

        assert_eq!(*count.lock().unwrap(), 3);
    }
}
