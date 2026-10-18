use crate::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer};
use crate::common::*;
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
        let event = match serde_json::from_slice::<Value>(body) {
            Ok(json) if json["type"].as_str().is_some_and(|kind| kind != "response.create") => return Vec::new(),
            Ok(json) => conversation_event(&json),
            Err(_) => unparsed(Side::Request),
        };
        vec![event.at(start, end)]
    }
}

fn conversation_event(json: &Value) -> TapEvent {
    let mut messages: Vec<(String, Vec<Value>)> = Vec::new();
    for (index, item) in json["input"].as_array().into_iter().flatten().enumerate() {
        let Some((role, blocks)) = input_item(index, item) else {
            continue;
        };
        match messages.last_mut() {
            Some((last, joined)) if *last == role => joined.extend(blocks),
            _ => messages.push((role, blocks)),
        }
    }
    let messages = messages
        .into_iter()
        .map(|(role, blocks)| message(&role, blocks))
        .collect();
    conversation(json["instructions"].clone(), json["tools"].clone(), messages)
}

fn input_item(index: usize, item: &Value) -> Option<(String, Vec<Value>)> {
    let location = index.to_string();
    match item["type"].as_str().unwrap_or("message") {
        "message" => {
            let role = if item["role"].as_str()? == "assistant" {
                "assistant"
            } else {
                "user"
            };
            Some((role.to_string(), content_parts(&location, &item["content"])))
        }
        "reasoning" => Some(("assistant".into(), vec![reasoning_of(item)])),
        "function_call" => {
            let input = item["arguments"].as_str().map_or(Value::Null, parse_arguments);
            Some((
                "assistant".into(),
                vec![tool_call_block(item["call_id"].clone(), item["name"].clone(), input)],
            ))
        }
        "function_call_output" => {
            let output = match &item["output"] {
                Value::Array(_) => Value::Array(content_parts(&location, &item["output"])),
                other => other.clone(),
            };
            Some((
                "user".into(),
                vec![tool_result_block(item["call_id"].clone(), output, false)],
            ))
        }
        _ => None,
    }
}

fn reasoning_of(item: &Value) -> Value {
    let summary: String = item["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect();
    match item["encrypted_content"].as_str() {
        Some(encrypted) if summary.is_empty() => encrypted_reasoning_block(json!(encrypted)),
        _ => reasoning_block(json!(summary), Value::Null),
    }
}

fn parse_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!(arguments))
}

fn content_parts(location: &str, content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => vec![text_block(text)],
        Value::Array(parts) => parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| match part["type"].as_str()? {
                "input_text" | "output_text" | "text" => Some(text_block(part["text"].as_str()?)),
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

fn image_of(location: &str, url: &str) -> Value {
    let (content_type, size) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .map(|(meta, data)| (meta.split(';').next().unwrap_or_default(), base64_size(data)))
        .unwrap_or_default();
    image_block(location, content_type, size)
}

fn stream_event(data: &str, start: u64, end: u64) -> Vec<TapEvent> {
    let Ok(json) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    let index = json["output_index"].as_u64().unwrap_or_default();
    let delta = || json["delta"].as_str().map(String::from);
    let events = match json["type"].as_str().unwrap_or_default() {
        "response.output_item.added" => vec![block_start(block_start_of(index, &json["item"]))],
        "response.output_text.delta" | "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            delta()
                .map(|text| block_delta(index, Delta::Text(text)))
                .into_iter()
                .collect()
        }
        "response.function_call_arguments.delta" => delta()
            .map(|json| block_delta(index, Delta::InputJson(json)))
            .into_iter()
            .collect(),
        "response.output_item.done" => {
            let encrypted = json["item"]["encrypted_content"].as_str();
            encrypted
                .map(|encrypted| block_delta(index, Delta::Encrypted(encrypted.into())))
                .into_iter()
                .chain([block_stop(index)])
                .collect()
        }
        "response.completed" | "response.incomplete" => {
            let response = &json["response"];
            vec![usage_event(&response["usage"]), reply_end(response["status"].clone())]
        }
        "response.failed" => vec![error(json["response"]["error"]["code"].clone())],
        "error" => vec![error(json["error"]["code"].clone())],
        _ => Vec::new(),
    };
    events.into_iter().map(|event| event.at(start, end)).collect()
}

fn block_start_of(index: u64, item: &Value) -> BlockStart {
    let (kind, id, name) = match item["type"].as_str().unwrap_or_default() {
        "message" => ("text", Value::Null, Value::Null),
        "reasoning" => ("reasoning", Value::Null, Value::Null),
        "function_call" => ("tool_call", item["call_id"].clone(), item["name"].clone()),
        _ => ("other", Value::Null, Value::Null),
    };
    BlockStart { index, kind, id, name }
}

fn usage_event(usage_json: &Value) -> TapEvent {
    usage(Usage {
        input: usage_json["input_tokens"].as_u64(),
        output: usage_json["output_tokens"].as_u64(),
        cache_read: usage_json["input_tokens_details"]["cached_tokens"].as_u64(),
        cache_write: None,
        reasoning: usage_json["output_tokens_details"]["reasoning_tokens"].as_u64(),
    })
}

fn stream_events(events: Vec<SseEvent>) -> Vec<TapEvent> {
    events
        .iter()
        .flat_map(|e| stream_event(&e.data, e.start, e.end))
        .collect()
}

impl ExchangeParser for CodexParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        match input {
            TapInput::RequestBody(chunk) => {
                self.request.push(&chunk);
                Vec::new()
            }
            TapInput::RequestEnd if self.request.unreadable() => vec![unparsed(Side::Request)],
            TapInput::RequestEnd if self.request.bytes().is_empty() => Vec::new(),
            TapInput::RequestEnd => self.request_events(self.request.bytes(), 0, self.request.bytes().len() as u64),
            TapInput::ResponseHead(head) => {
                self.response_encoded = is_encoded(&head.headers);
                Vec::new()
            }
            TapInput::ResponseBody(chunk) if !self.response_encoded => stream_events(self.sse.push(&chunk)),
            TapInput::ResponseEnd if self.response_encoded => vec![unparsed(Side::Reply)],
            TapInput::ResponseEnd => stream_events(self.sse.finish()),
            TapInput::WsMessage {
                from_client: true,
                text,
                start,
                end,
            } => self.request_events(text.as_bytes(), start, end),
            TapInput::WsMessage { text, start, end, .. } => stream_event(&text, start, end),
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
    fn maps_an_http_request_split_across_chunks_to_a_conversation() {
        // It keeps instructions as system, tools, and merges consecutive same-role items
        let body = br#"{"model":"m1","instructions":"sys","tools":[{"type":"function","name":"shell"}],"input":[
            {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
            {"type":"reasoning","summary":[{"type":"summary_text","text":"think"}]},
            {"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"cmd\":\"ls\"}"},
            {"type":"function_call_output","call_id":"c1","output":"out"},
            {"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AAAA"}]}]}"#;
        let mut parser = parser();

        for piece in body.chunks(41) {
            parser.feed(TapInput::RequestBody(Bytes::copy_from_slice(piece)));
        }
        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(kinds(&events), ["conversation"]);
        assert_eq!(events[0].payload["system"], "sys");
        assert_eq!(
            events[0].payload["tools"],
            json!([{"type": "function", "name": "shell"}])
        );
        assert_eq!(events[0].payload.get("model"), None);
        assert_eq!(
            events[0].payload["messages"],
            json!([
                message("user", vec![text_block("hi")]),
                message(
                    "assistant",
                    vec![
                        reasoning_block(json!("think"), Value::Null),
                        tool_call_block(json!("c1"), json!("shell"), json!({"cmd": "ls"})),
                    ]
                ),
                message(
                    "user",
                    vec![
                        tool_result_block(json!("c1"), json!("out"), false),
                        image_block("4:0", "image/png", 3)
                    ]
                ),
            ])
        );
    }

    #[test]
    fn maps_non_assistant_roles_to_user() {
        // It keeps the common format to user and assistant, folding developer into user
        let events = parser().request_events(
            br#"{"input":[{"type":"message","role":"developer","content":"d"},{"type":"message","role":"user","content":"u"}]}"#,
            0,
            1,
        );

        assert_eq!(
            events[0].payload["messages"],
            json!([message("user", vec![text_block("d"), text_block("u")])])
        );
    }

    #[test]
    fn ignores_websocket_frames_that_are_not_requests() {
        // A cancel frame opens no conversation and so leaves no phantom record
        let events = parser().request_events(br#"{"type":"response.cancel"}"#, 0, 1);

        assert!(events.is_empty());
    }

    #[test]
    fn maps_a_websocket_request_message_like_an_http_body() {
        // It reads response.create text from the client the same way
        let mut parser = parser();
        let ws = |text: &str| TapInput::WsMessage {
            from_client: true,
            text: text.into(),
            start: 3,
            end: 9,
        };

        let events = parser.feed(ws(
            r#"{"type":"response.create","instructions":"s","input":[{"role":"user","content":"hi"}]}"#,
        ));

        assert_eq!(kinds(&events), ["conversation"]);
        assert_eq!((events[0].start, events[0].end), (3, 9));
        assert_eq!(
            events[0].payload["messages"],
            json!([message("user", vec![text_block("hi")])])
        );
    }

    #[test]
    fn maps_sse_events_split_across_chunks_with_a_partial_final_line() {
        // It yields blocks, deltas and usage in order, and flushes the last unterminated event
        let stream = concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs1\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"delta\":\"hm\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"enc\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":1,\"delta\":\"ok\"}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":2,\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"shell\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":2,\"delta\":\"{}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":1}}}}",
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
                "block_start",
                "block_delta",
                "block_delta",
                "block_stop",
                "block_start",
                "block_delta",
                "block_start",
                "block_delta",
                "usage",
                "reply_end"
            ]
        );
        assert_eq!(events[0].payload, json!({"index": 0, "kind": "reasoning"}));
        assert_eq!(events[1].payload, json!({"index": 0, "text": "hm"}));
        assert_eq!(events[2].payload, json!({"index": 0, "encrypted": "enc"}));
        assert_eq!(events[4].payload, json!({"index": 1, "kind": "text"}));
        assert_eq!(
            events[6].payload,
            json!({"index": 2, "kind": "tool_call", "id": "c1", "name": "shell"})
        );
        assert_eq!(events[7].payload, json!({"index": 2, "input_json": "{}"}));
        assert_eq!(events[8].payload["input"], 3);
        assert_eq!(events[8].payload["cache_read"], 2);
        assert_eq!(events[8].payload["reasoning"], 1);
        assert_eq!(events[9].payload, json!({"stop_reason": "completed"}));
    }

    #[test]
    fn maps_websocket_server_messages_like_sse_events() {
        // It reads a server message the same way as an SSE data line
        let mut parser = parser();

        let events = parser.feed(TapInput::WsMessage {
            from_client: false,
            text: r#"{"type":"response.output_text.delta","output_index":1,"delta":"x"}"#.into(),
            start: 0,
            end: 1,
        });

        assert_eq!(kinds(&events), ["block_delta"]);
        assert_eq!(events[0].payload, json!({"index": 1, "text": "x"}));
    }

    #[test]
    fn reports_a_failure_by_code_only() {
        // It leaves the upstream message text out
        let mut parser = parser();

        let events = parser.feed(TapInput::ResponseBody(Bytes::from_static(
            b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit\",\"message\":\"slow\"}}}\n\n",
        )));

        assert_eq!(events[0].payload, json!({"error_type": "rate_limit"}));
    }

    #[test]
    fn flags_a_request_body_over_the_cap_as_unparsed() {
        // It reports the request as unparsed rather than reading a partial body
        let mut parser = parser();
        parser.feed(TapInput::RequestBody(Bytes::from(vec![b' '; MAX_REQUEST_BODY])));
        parser.feed(TapInput::RequestBody(Bytes::from_static(b"{}")));

        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(kinds(&events), ["unparsed"]);
        assert_eq!(events[0].payload, json!({"side": "request"}));
    }

    #[test]
    fn an_encoded_reply_is_reported_as_unparsed() {
        // It emits the unparsed marker for the reply side
        let mut parser = parser();
        parser.feed(TapInput::ResponseHead(crate::adapter::ResponseHead {
            status: 200,
            headers: vec![("content-encoding".into(), "gzip".into())],
        }));
        parser.feed(TapInput::ResponseBody(Bytes::from_static(b"zz")));

        let events = parser.feed(TapInput::ResponseEnd);

        assert_eq!(kinds(&events), ["unparsed"]);
        assert_eq!(events[0].payload, json!({"side": "reply"}));
    }

    #[test]
    fn skips_non_json_stream_data() {
        // It ignores events that are not JSON objects with a type
        let mut parser = parser();

        let events = parser.feed(TapInput::ResponseBody(Bytes::from_static(b"data: [DONE]\n\n")));

        assert!(events.is_empty());
    }
}
