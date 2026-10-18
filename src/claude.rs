use crate::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer};
use crate::common::*;
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
mod tests {
    use super::*;
    use crate::adapter::ResponseHead;
    use bytes::Bytes;
    use serde_json::json;

    fn head(path: &str) -> RequestHead {
        RequestHead {
            method: "POST".into(),
            host: "api.anthropic.com".into(),
            path: path.into(),
            headers: Vec::new(),
        }
    }

    fn parser() -> Box<dyn ExchangeParser> {
        ClaudeAdapter::default().claim(&head("/v1/messages")).unwrap()
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

    fn stream(wire: &[u8], chunk: usize) -> Vec<TapEvent> {
        let mut parser = parser();
        let mut events = parser.feed(sse_head());
        for piece in wire.chunks(chunk) {
            events.extend(parser.feed(TapInput::ResponseBody(Bytes::copy_from_slice(piece))));
        }
        events.extend(parser.feed(TapInput::ResponseEnd));
        events
    }

    fn request(body: &[u8], chunk: usize) -> Vec<TapEvent> {
        let mut parser = parser();
        for piece in body.chunks(chunk) {
            parser.feed(TapInput::RequestBody(Bytes::copy_from_slice(piece)));
        }
        parser.feed(TapInput::RequestEnd)
    }

    fn kinds(events: &[TapEvent]) -> Vec<&str> {
        events.iter().map(|e| e.kind.as_str()).collect()
    }

    #[test]
    fn claims_the_messages_endpoint_with_or_without_a_query_and_nothing_else() {
        // It reads only the model endpoint
        let adapter = ClaudeAdapter::default();

        assert!(adapter.claim(&head("/v1/messages")).is_some());
        assert!(adapter.claim(&head("/v1/messages?beta=true")).is_some());
        assert!(adapter.claim(&head("/v1/messages/count_tokens")).is_none());
        assert!(adapter.claim(&head("/api/oauth/profile")).is_none());
    }

    #[test]
    fn maps_a_messages_request_split_across_chunks_to_a_conversation() {
        // It keeps system, tools and every message with blocks in the common kinds
        let body = br#"{"model":"m1","stream":true,"max_tokens":64,"system":"sys","tools":[{"name":"Bash"}],"messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"sig"},{"type":"text","text":"ok"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"out","is_error":true},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]}"#;

        let events = request(body, 37);

        assert_eq!(kinds(&events), ["conversation"]);
        let payload = &events[0].payload;
        assert_eq!(payload["system"], "sys");
        assert_eq!(payload["tools"], json!([{"name": "Bash"}]));
        assert_eq!(payload.get("model"), None);
        assert_eq!(
            payload["messages"],
            json!([
                message("user", vec![text_block("hi")]),
                message(
                    "assistant",
                    vec![
                        reasoning_block(json!("hmm"), json!("sig")),
                        text_block("ok"),
                        tool_call_block(json!("t1"), json!("Bash"), json!({"command": "ls"})),
                    ]
                ),
                message(
                    "user",
                    vec![
                        tool_result_block(json!("t1"), json!("out"), true),
                        image_block("2:1", "image/png", 3)
                    ]
                ),
            ])
        );
    }

    #[test]
    fn an_image_inside_a_tool_result_becomes_a_reference() {
        // It never copies the bytes of a nested image
        let body = br#"{"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"see"},{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"QUJDREVGRw=="}}]}]}]}"#;

        let events = request(body, 64);

        let output = &events[0].payload["messages"][0]["blocks"][0]["output"];
        assert_eq!(
            *output,
            json!([text_block("see"), image_block("0:0:1", "image/jpeg", 7)])
        );
        assert!(!events[0].payload.to_string().contains("QUJDREVGRw"));
    }

    #[test]
    fn an_encoded_or_oversized_request_is_reported_as_unparsed() {
        // It emits the unparsed marker for the request side instead of guessing
        let encoded = RequestHead {
            headers: vec![("content-encoding".into(), "gzip".into())],
            ..head("/v1/messages")
        };
        let mut parser = ClaudeAdapter::default().claim(&encoded).unwrap();
        parser.feed(TapInput::RequestBody(Bytes::from_static(b"zz")));

        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(kinds(&events), ["unparsed"]);
        assert_eq!(events[0].payload, json!({"side": "request"}));
    }

    #[test]
    fn maps_a_text_stream_split_mid_line_and_mid_json() {
        // It yields usage, one text block and the reply end once, however the bytes are chunked
        let wire = sse(&[
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","model":"m1","usage":{"input_tokens":9,"output_tokens":1,"cache_read_input_tokens":5,"cache_creation_input_tokens":2}}}"#,
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

        let events = stream(&wire, 11);

        assert_eq!(
            kinds(&events),
            [
                "usage",
                "block_start",
                "block_delta",
                "block_stop",
                "usage",
                "reply_end"
            ]
        );
        assert_eq!(events[0].payload["input"], 9);
        assert_eq!(events[0].payload["cache_read"], 5);
        assert_eq!(events[0].payload["cache_write"], 2);
        assert_eq!(events[1].payload, json!({"index": 0, "kind": "text"}));
        assert_eq!(events[2].payload, json!({"index": 0, "text": "ok"}));
        assert_eq!(events[4].payload["output"], 4);
        assert_eq!(events[5].payload, json!({"stop_reason": "end_turn"}));
    }

    #[test]
    fn keeps_tool_use_name_id_and_json_fragments() {
        // It reports the tool block's name and id, and each fragment in order
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

        let events = stream(&wire, 5);

        assert_eq!(
            events[0].payload,
            json!({"index": 1, "kind": "tool_call", "id": "toolu_1", "name": "Bash"})
        );
        let fragments: String = events[1..]
            .iter()
            .map(|e| e.payload["input_json"].as_str().unwrap())
            .collect();
        assert_eq!(fragments, r#"{"command":"ls"}"#);
    }

    #[test]
    fn maps_thinking_to_reasoning_with_its_signature() {
        // It sends thinking text and signature deltas to a reasoning block
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

        let events = stream(&wire, 9);

        assert_eq!(events[0].payload, json!({"index": 0, "kind": "reasoning"}));
        assert_eq!(events[1].payload, json!({"index": 0, "text": "hmm"}));
        assert_eq!(events[2].payload, json!({"index": 0, "signature": "sig=="}));
        assert_eq!(events[3].payload, json!({"index": 1, "kind": "reasoning"}));
    }

    #[test]
    fn a_block_outside_the_format_keeps_its_index_under_the_other_kind() {
        // It lets the Assembler leave it out while later indexes keep their order
        let wire = sse(&[(
            "content_block_start",
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"server_tool_use","id":"s1","name":"web_search"}}"#,
        )]);

        let events = stream(&wire, 64);

        assert_eq!(events[0].payload, json!({"index": 2, "kind": "other"}));
    }

    #[test]
    fn reports_a_stream_error_by_type_only() {
        // It leaves the upstream message text out
        let wire = sse(&[(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
        )]);

        let events = stream(&wire, 64);

        assert_eq!(kinds(&events), ["error"]);
        assert_eq!(events[0].payload, json!({"error_type": "overloaded_error"}));
    }

    #[test]
    fn a_reply_that_is_encoded_or_not_a_stream_is_reported_as_unparsed() {
        // It emits the unparsed marker for the reply side
        let mut parser = parser();
        parser.feed(TapInput::ResponseHead(ResponseHead {
            status: 200,
            headers: vec![("content-encoding".into(), "gzip".into())],
        }));
        parser.feed(TapInput::ResponseBody(Bytes::from_static(b"zz")));

        let events = parser.feed(TapInput::ResponseEnd);

        assert_eq!(kinds(&events), ["unparsed"]);
        assert_eq!(events[0].payload, json!({"side": "reply"}));
    }
}
