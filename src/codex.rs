use crate::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{event, is_encoded, tool_names, RequestBuffer};
use crate::sse::{SseEvent, SseSplitter};
use serde_json::{json, Value};

pub struct CodexAdapter {
    hosts: Vec<String>,
}

impl CodexAdapter {
    pub fn new(hosts: Vec<String>) -> Self {
        Self { hosts }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new(vec!["chatgpt.com".into(), "api.openai.com".into()])
    }
}

impl Adapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn hosts(&self) -> &[String] {
        &self.hosts
    }

    fn claim(&self, head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        head.path
            .ends_with("/responses")
            .then(|| Box::new(CodexParser::new(is_encoded(&head.headers))) as Box<dyn ExchangeParser>)
    }
}

pub struct CodexParser {
    request: RequestBuffer,
    response_encoded: bool,
    sse: SseSplitter,
}

impl CodexParser {
    fn new(request_encoded: bool) -> Self {
        Self {
            request: RequestBuffer::new(request_encoded),
            response_encoded: false,
            sse: SseSplitter::default(),
        }
    }

    fn request_events(&self, body: &[u8], start: u64, end: u64) -> Vec<TapEvent> {
        let payload = match serde_json::from_slice::<Value>(body) {
            Ok(json) => json!({
                "model": json["model"],
                "input_items": json["input"].as_array().map(Vec::len),
                "tools": tool_names(&json["tools"]),
            }),
            Err(_) => return vec![event("request_unparsed", json!({"bytes": body.len()}), start, end)],
        };
        vec![event("request", payload, start, end)]
    }
}

fn stream_event(data: &str, start: u64, end: u64) -> Option<TapEvent> {
    let json: Value = serde_json::from_str(data).ok()?;
    let kind = json["type"].as_str()?;
    let payload = if kind.ends_with(".delta") {
        json!({"delta": json["delta"], "item_id": json["item_id"], "output_index": json["output_index"]})
    } else if let Some(response) = json.get("response") {
        json!({"id": response["id"], "model": response["model"], "status": response["status"], "usage": response["usage"]})
    } else if let Some(item) = json.get("item") {
        json!({"item_type": item["type"], "item_id": item["id"], "name": item["name"]})
    } else {
        json!({})
    };
    Some(event(kind, payload, start, end))
}

fn stream_events(events: Vec<SseEvent>) -> Vec<TapEvent> {
    events
        .iter()
        .filter_map(|e| stream_event(&e.data, e.start, e.end))
        .collect()
}

impl ExchangeParser for CodexParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        match input {
            TapInput::RequestBody(chunk) => {
                self.request.push(&chunk);
                Vec::new()
            }
            TapInput::RequestEnd => match self.request.unreadable() {
                Some(unreadable) => vec![unreadable],
                None if self.request.bytes().is_empty() => Vec::new(),
                None => self.request_events(self.request.bytes(), 0, self.request.bytes().len() as u64),
            },
            TapInput::ResponseHead(head) => {
                self.response_encoded = is_encoded(&head.headers);
                Vec::new()
            }
            TapInput::ResponseBody(chunk) if !self.response_encoded => stream_events(self.sse.push(&chunk)),
            TapInput::ResponseEnd => stream_events(self.sse.finish()),
            TapInput::WsMessage {
                from_client: true,
                text,
                start,
                end,
            } => self.request_events(text.as_bytes(), start, end),
            TapInput::WsMessage { text, start, end, .. } => stream_event(&text, start, end).into_iter().collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::MAX_REQUEST_BODY;
    use bytes::Bytes;

    fn parser() -> CodexParser {
        CodexParser::new(false)
    }

    fn kinds(events: &[TapEvent]) -> Vec<&str> {
        events.iter().map(|e| e.kind.as_str()).collect()
    }

    #[test]
    fn claims_only_responses_paths() {
        // It claims the Responses endpoint and nothing else
        let adapter = CodexAdapter::default();
        let head = |path: &str| RequestHead {
            method: "POST".into(),
            host: "chatgpt.com".into(),
            path: path.into(),
            headers: Vec::new(),
        };

        assert!(adapter.claim(&head("/backend-api/codex/responses")).is_some());
        assert!(adapter.claim(&head("/backend-api/wham/usage")).is_none());
    }

    #[test]
    fn summarises_the_request_body_split_across_chunks() {
        // It reports model, input count and tool names once the body ends
        let body =
            br#"{"model":"m1","input":[{},{}],"tools":[{"type":"function","name":"shell"},{"type":"web_search"}]}"#;
        let mut parser = parser();

        let early = parser.feed(TapInput::RequestBody(Bytes::copy_from_slice(&body[..20])));
        parser.feed(TapInput::RequestBody(Bytes::copy_from_slice(&body[20..])));
        let events = parser.feed(TapInput::RequestEnd);

        assert!(early.is_empty());
        assert_eq!(kinds(&events), ["request"]);
        assert_eq!(events[0].payload["model"], "m1");
        assert_eq!(events[0].payload["input_items"], 2);
        assert_eq!(events[0].payload["tools"], json!(["shell", "web_search"]));
    }

    #[test]
    fn parses_sse_deltas_split_across_chunks_with_a_partial_final_line() {
        // It yields deltas and usage in order, and flushes the last unterminated event
        let stream = concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"model\":\"m1\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\\\"a\\\"\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}",
        )
        .as_bytes();
        let mut parser = parser();
        let mut events = Vec::new();

        for chunk in stream.chunks(13) {
            events.extend(parser.feed(TapInput::ResponseBody(Bytes::copy_from_slice(chunk))));
        }
        events.extend(parser.feed(TapInput::ResponseEnd));

        assert_eq!(
            kinds(&events),
            [
                "response.created",
                "response.output_text.delta",
                "response.function_call_arguments.delta",
                "response.completed"
            ]
        );
        assert_eq!(events[1].payload["delta"], "ok");
        assert_eq!(events[3].payload["usage"]["output_tokens"], 1);
    }

    #[test]
    fn flags_a_request_body_over_the_cap_as_truncated() {
        // It reports the request as unparsed and truncated rather than summarising a partial body
        let mut parser = parser();
        parser.feed(TapInput::RequestBody(Bytes::from(vec![b' '; MAX_REQUEST_BODY])));
        parser.feed(TapInput::RequestBody(Bytes::from_static(b"{}")));

        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(kinds(&events), ["request_unparsed"]);
        assert_eq!(events[0].payload["truncated"], true);
    }

    #[test]
    fn parses_websocket_messages_in_both_directions() {
        // It reads a client request message and a server delta message alike
        let mut parser = parser();
        let ws = |from_client, text: &str| TapInput::WsMessage {
            from_client,
            text: text.into(),
            start: 0,
            end: 1,
        };

        let request = parser.feed(ws(true, r#"{"type":"response.create","model":"m1","input":[{}]}"#));
        let delta = parser.feed(ws(
            false,
            r#"{"type":"response.reasoning_summary_text.delta","delta":"x"}"#,
        ));

        assert_eq!(request[0].payload["input_items"], 1);
        assert_eq!(kinds(&delta), ["response.reasoning_summary_text.delta"]);
    }

    #[test]
    fn skips_non_json_stream_data() {
        // It ignores events that are not JSON objects with a type
        let mut parser = parser();

        let events = parser.feed(TapInput::ResponseBody(Bytes::from_static(b"data: [DONE]\n\n")));

        assert!(events.is_empty());
    }
}
