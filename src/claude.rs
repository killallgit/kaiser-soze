use crate::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{event, is_encoded, tool_names, RequestBuffer};
use crate::sse::{SseEvent, SseSplitter};
use serde_json::{json, Value};

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
        Some(Box::new(ClaudeParser::new(head)))
    }
}

const MESSAGES_PATH: &str = "/v1/messages";
const DELTA_FIELDS: [&str; 4] = ["text", "thinking", "signature", "partial_json"];

pub struct ClaudeParser {
    path: String,
    beta: Option<String>,
    request: RequestBuffer,
    status: u16,
    response_sse: bool,
    response_encoded: bool,
    response_bytes: u64,
    sse: SseSplitter,
}

impl ClaudeParser {
    fn new(head: &RequestHead) -> Self {
        let beta = head
            .headers
            .iter()
            .find(|(name, _)| name == "anthropic-beta")
            .map(|(_, value)| value.clone());
        Self {
            path: head.path.clone(),
            beta,
            request: RequestBuffer::new(is_encoded(&head.headers)),
            status: 0,
            response_sse: false,
            response_encoded: false,
            response_bytes: 0,
            sse: SseSplitter::default(),
        }
    }

    fn request_events(&self) -> Vec<TapEvent> {
        if let Some(unreadable) = self.request.unreadable() {
            return vec![unreadable];
        }
        let body = self.request.bytes();
        let payload = match serde_json::from_slice::<Value>(body) {
            Ok(json) if self.path == MESSAGES_PATH => json!({
                "path": self.path,
                "model": json["model"],
                "stream": json["stream"],
                "messages": json["messages"].as_array().map(Vec::len),
                "tools": tool_names(&json["tools"]),
                "system": !json["system"].is_null(),
                "max_tokens": json["max_tokens"],
                "anthropic_beta": self.beta,
            }),
            _ => json!({"path": self.path, "bytes": body.len()}),
        };
        vec![event("request", payload, 0, body.len() as u64)]
    }

    fn response_summary(&self) -> Vec<TapEvent> {
        let payload = json!({"status": self.status, "bytes": self.response_bytes, "encoded": self.response_encoded});
        vec![event("response", payload, 0, self.response_bytes)]
    }
}

fn stream_event(data: &str, start: u64, end: u64) -> Option<TapEvent> {
    let json: Value = serde_json::from_str(data).ok()?;
    let kind = json["type"].as_str()?;
    let payload = match kind {
        "message_start" => {
            let message = &json["message"];
            json!({"id": message["id"], "model": message["model"], "usage": message["usage"]})
        }
        "content_block_start" => {
            let block = &json["content_block"];
            json!({"index": json["index"], "block_type": block["type"], "name": block["name"], "id": block["id"]})
        }
        "content_block_delta" => {
            let delta = &json["delta"];
            let content = DELTA_FIELDS.iter().find_map(|field| delta.get(field));
            json!({"index": json["index"], "delta_type": delta["type"], "delta": content})
        }
        "content_block_stop" => json!({"index": json["index"]}),
        "message_delta" => json!({"stop_reason": json["delta"]["stop_reason"], "usage": json["usage"]}),
        "error" => json!({"error_type": json["error"]["type"], "message": json["error"]["message"]}),
        _ => json!({}),
    };
    Some(event(kind, payload, start, end))
}

fn stream_events(events: Vec<SseEvent>) -> Vec<TapEvent> {
    events
        .iter()
        .filter_map(|e| stream_event(&e.data, e.start, e.end))
        .collect()
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
                self.status = head.status;
                self.response_encoded = is_encoded(&head.headers);
                self.response_sse = head
                    .headers
                    .iter()
                    .any(|(name, value)| name == "content-type" && value.contains("event-stream"));
                Vec::new()
            }
            TapInput::ResponseBody(chunk) => {
                self.response_bytes += chunk.len() as u64;
                if self.response_sse && !self.response_encoded {
                    stream_events(self.sse.push(&chunk))
                } else {
                    Vec::new()
                }
            }
            TapInput::ResponseEnd if self.response_sse && !self.response_encoded => stream_events(self.sse.finish()),
            TapInput::ResponseEnd => self.response_summary(),
            TapInput::WsMessage { .. } => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::ResponseHead;
    use bytes::Bytes;
    use serde_json::json;

    fn head(path: &str, beta: Option<&str>) -> RequestHead {
        RequestHead {
            method: "POST".into(),
            host: "api.anthropic.com".into(),
            path: path.into(),
            headers: beta
                .map(|value| vec![("anthropic-beta".to_string(), value.to_string())])
                .unwrap_or_default(),
        }
    }

    fn parser_for(path: &str, beta: Option<&str>) -> Box<dyn ExchangeParser> {
        ClaudeAdapter::default().claim(&head(path, beta)).unwrap()
    }

    fn sse_head() -> TapInput {
        TapInput::ResponseHead(ResponseHead {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
        })
    }

    fn sse(events: &[(&str, &str)]) -> Vec<u8> {
        events
            .iter()
            .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn stream(path: &str, wire: &[u8], chunk: usize) -> Vec<TapEvent> {
        let mut parser = parser_for(path, None);
        let mut events = parser.feed(sse_head());
        for piece in wire.chunks(chunk) {
            events.extend(parser.feed(TapInput::ResponseBody(Bytes::copy_from_slice(piece))));
        }
        events.extend(parser.feed(TapInput::ResponseEnd));
        events
    }

    fn kinds(events: &[TapEvent]) -> Vec<&str> {
        events.iter().map(|e| e.kind.as_str()).collect()
    }

    #[test]
    fn summarises_a_messages_request_split_across_chunks() {
        // It reports model, stream flag, counts, tool names, system presence, max_tokens and the beta header
        let body = br#"{"model":"m1","stream":true,"max_tokens":64,"system":"s","messages":[{},{},{}],"tools":[{"name":"Bash"},{"name":"Read"}]}"#;
        let mut parser = parser_for("/v1/messages", Some("beta-a,beta-b"));

        parser.feed(TapInput::RequestBody(Bytes::copy_from_slice(&body[..30])));
        parser.feed(TapInput::RequestBody(Bytes::copy_from_slice(&body[30..])));
        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(kinds(&events), ["request"]);
        assert_eq!(
            events[0].payload,
            json!({
                "path": "/v1/messages", "model": "m1", "stream": true, "messages": 3,
                "tools": ["Bash", "Read"], "system": true, "max_tokens": 64,
                "anthropic_beta": "beta-a,beta-b",
            })
        );
    }

    #[test]
    fn other_paths_get_a_request_and_response_summary_only() {
        // It records sizes and status for count_tokens, not the body
        let mut parser = parser_for("/v1/messages/count_tokens", None);
        parser.feed(TapInput::RequestBody(Bytes::from_static(b"{\"model\":\"m1\"}")));
        let request = parser.feed(TapInput::RequestEnd);
        parser.feed(TapInput::ResponseHead(ResponseHead {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
        }));
        parser.feed(TapInput::ResponseBody(Bytes::from_static(b"{\"input_tokens\":3}")));
        let response = parser.feed(TapInput::ResponseEnd);

        assert_eq!(request[0].payload["path"], "/v1/messages/count_tokens");
        assert_eq!(request[0].payload["bytes"], 14);
        assert_eq!(kinds(&response), ["response"]);
        assert_eq!(response[0].payload["status"], 200);
        assert_eq!(response[0].payload["bytes"], 18);
    }

    #[test]
    fn parses_a_text_stream_split_mid_line_and_mid_json() {
        // It yields every Anthropic event once, in order, however the bytes are chunked
        let wire = sse(&[
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","model":"m1","usage":{"input_tokens":9,"output_tokens":1}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            ("ping", r#"{"type":"ping"}"#),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
            ),
            ("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]);

        let events = stream("/v1/messages", &wire, 11);

        assert_eq!(
            kinds(&events),
            [
                "message_start",
                "content_block_start",
                "ping",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(events[0].payload["id"], "msg_1");
        assert_eq!(events[0].payload["usage"]["input_tokens"], 9);
        assert_eq!(events[1].payload["block_type"], "text");
        assert_eq!(events[3].payload["delta_type"], "text_delta");
        assert_eq!(events[3].payload["delta"], "ok");
        assert_eq!(events[5].payload["stop_reason"], "end_turn");
        assert_eq!(events[5].payload["usage"]["output_tokens"], 4);
    }

    #[test]
    fn keeps_tool_use_name_id_and_json_fragments() {
        // It reports the tool block's name and id, and each partial_json fragment in order
        let wire = sse(&[
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"co"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"mmand\":\"ls\"}"}}"#,
            ),
        ]);

        let events = stream("/v1/messages", &wire, 5);

        assert_eq!(events[0].payload["block_type"], "tool_use");
        assert_eq!(events[0].payload["name"], "Bash");
        assert_eq!(events[0].payload["id"], "toolu_1");
        let fragments: String = events[1..]
            .iter()
            .map(|e| e.payload["delta"].as_str().unwrap())
            .collect();
        assert_eq!(fragments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn keeps_thinking_text_and_signature_deltas() {
        // It reports thinking and signature deltas under their delta types
        let wire = sse(&[
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig=="}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"x"}}"#,
            ),
        ]);

        let events = stream("/v1/messages", &wire, 9);

        assert_eq!(events[0].payload["block_type"], "thinking");
        assert_eq!(
            (
                events[1].payload["delta_type"].as_str(),
                events[1].payload["delta"].as_str()
            ),
            (Some("thinking_delta"), Some("hmm"))
        );
        assert_eq!(
            (
                events[2].payload["delta_type"].as_str(),
                events[2].payload["delta"].as_str()
            ),
            (Some("signature_delta"), Some("sig=="))
        );
        assert_eq!(events[3].payload["block_type"], "redacted_thinking");
    }

    #[test]
    fn reports_stream_errors() {
        // It surfaces an error event's type and message
        let wire = sse(&[(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        )]);

        let events = stream("/v1/messages", &wire, 64);

        assert_eq!(kinds(&events), ["error"]);
        assert_eq!(events[0].payload["error_type"], "overloaded_error");
        assert_eq!(events[0].payload["message"], "busy");
    }
}
