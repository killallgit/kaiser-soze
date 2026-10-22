use crate::adapter::{header, Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use crate::body::{is_encoded, RequestBuffer, MAX_REQUEST_BODY};
use crate::common::*;
use crate::graph::Identity;
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

    fn identity(&self, head: &RequestHead) -> Identity {
        let owned = |name| header(head, name).map(str::to_string);
        let turn_metadata =
            header(head, "x-codex-turn-metadata").and_then(|json| serde_json::from_str::<Value>(json).ok());
        Identity {
            session: owned("session-id"),
            thread: owned("thread-id"),
            parent_thread: owned("x-codex-parent-thread-id"),
            turn: turn_metadata.and_then(|metadata| metadata["turn_id"].as_str().map(str::to_string)),
        }
    }
}

pub struct CodexParser {
    history: History,
    request: RequestBuffer,
    response_encoded: bool,
    sse: SseSplitter,
}

impl CodexParser {
    fn new(request_encoded: bool) -> Self {
        Self {
            history: History::default(),
            request: RequestBuffer::new(request_encoded),
            response_encoded: false,
            sse: SseSplitter::default(),
        }
    }

    fn request_events(&mut self, body: &[u8], start: u64, end: u64) -> Vec<TapEvent> {
        let event = match serde_json::from_slice::<Value>(body) {
            Ok(json) if json["type"].as_str().is_some_and(|kind| kind != "response.create") => return Vec::new(),
            Ok(json) => self.history.join(&json),
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

/// The running conversation of one socket. Codex sends only the new items with
/// `previous_response_id`, so the parser joins them to what it stored. A request it cannot
/// chain (joined mid-socket, dropped frame, past the bound) is emitted as sent with
/// `partial: true`, and nothing chains from it until a request starts a new history.
#[derive(Default)]
struct History {
    last_reply: Option<String>,
    system: Value,
    tools: Value,
    messages: Vec<Value>,
    bytes: usize,
    broken: bool,
    reply_id: Option<String>,
    reply_items: Vec<(String, Vec<Value>)>,
}

impl History {
    fn join(&mut self, json: &Value) -> TapEvent {
        let (system, tools, messages) = read_request(json);
        let previous = json["previous_response_id"].as_str();
        let partial = previous.is_some() && (self.broken || previous != self.last_reply.as_deref());
        let mut event = if partial {
            self.broken = true;
            conversation(system, tools, messages)
        } else {
            if previous.is_none() {
                *self = History::default();
            }
            let system = if system.is_null() { self.system.clone() } else { system };
            let tools = if tools.is_null() { self.tools.clone() } else { tools };
            self.bytes += size_of(&messages);
            append(&mut self.messages, messages);
            (self.system, self.tools) = (system.clone(), tools.clone());
            let event = conversation(system, tools, self.messages.clone());
            self.bound();
            event
        };
        if partial {
            event.payload["partial"] = json!(true);
        }
        if json["generate"] == json!(false) {
            event.payload["warmup"] = json!(true);
        }
        event
    }

    fn watch(&mut self, data: &str) {
        let Ok(json) = serde_json::from_str::<Value>(data) else {
            return;
        };
        match json["type"].as_str().unwrap_or_default() {
            "response.created" => {
                self.reply_id = json["response"]["id"].as_str().map(String::from);
                self.reply_items.clear();
            }
            "response.output_item.done" => {
                let index = json["output_index"].as_u64().unwrap_or_default() as usize;
                self.reply_items.extend(input_item(index, &json["item"]));
            }
            "response.completed" | "response.incomplete" => self.commit(&json["response"]["output"]),
            _ => {}
        }
    }

    fn commit(&mut self, output: &Value) {
        let items = std::mem::take(&mut self.reply_items);
        let id = self.reply_id.take();
        let output_len = output.as_array().map_or(0, Vec::len);
        // shortcut: a dropped output_item.done frame is caught only when the reply lists its output
        if self.broken || (output_len > 0 && output_len != items.len()) {
            self.broken = true;
            return;
        }
        let reply: Vec<Value> = items.into_iter().map(|(role, blocks)| message(&role, blocks)).collect();
        self.bytes += size_of(&reply);
        append(&mut self.messages, reply);
        self.last_reply = id;
        self.bound();
    }

    fn bound(&mut self) {
        if self.bytes > MAX_REQUEST_BODY {
            self.broken = true;
        }
        if self.broken {
            self.messages = Vec::new();
        }
    }
}

fn size_of(messages: &[Value]) -> usize {
    messages.iter().map(|m| m.to_string().len()).sum()
}

/// Adds messages, joining one to the last when they share a role.
fn append(into: &mut Vec<Value>, more: Vec<Value>) {
    for next in more {
        match into.last_mut() {
            Some(last) if last["role"] == next["role"] => {
                let blocks = next["blocks"].as_array().cloned().unwrap_or_default();
                if let Some(joined) = last["blocks"].as_array_mut() {
                    joined.extend(blocks);
                }
            }
            _ => into.push(next),
        }
    }
}

fn read_request(json: &Value) -> (Value, Value, Vec<Value>) {
    let mut messages: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    let mut tools = Value::Null;
    for (index, item) in json["input"].as_array().into_iter().flatten().enumerate() {
        match (item["type"].as_str().unwrap_or("message"), item["role"].as_str()) {
            ("additional_tools", _) => tools = item["tools"].clone(),
            ("message", Some("developer" | "system")) => system.extend(system_entries(&item["content"])),
            _ => {
                if let Some((role, blocks)) = input_item(index, item) {
                    append(&mut messages, vec![message(&role, blocks)]);
                }
            }
        }
    }
    let system = match (&json["instructions"], system.is_empty()) {
        (Value::Null, false) => Value::Array(system),
        (instructions, _) => instructions.clone(),
    };
    let tools = if json["tools"].is_null() {
        tools
    } else {
        json["tools"].clone()
    };
    (system, tools, messages)
}

fn system_entries(content: &Value) -> Vec<Value> {
    let texts: Vec<&str> = match content {
        Value::String(text) => vec![text],
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect(),
        _ => Vec::new(),
    };
    texts
        .into_iter()
        .map(|text| json!({"type": "text", "text": text}))
        .collect()
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
        kind @ ("function_call" | "custom_tool_call") => {
            let input = if kind == "function_call" {
                item["arguments"].as_str().map_or(Value::Null, parse_arguments)
            } else {
                item["input"].clone()
            };
            Some((
                "assistant".into(),
                vec![tool_call_block(item["call_id"].clone(), item["name"].clone(), input)],
            ))
        }
        "function_call_output" | "custom_tool_call_output" => {
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
        "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => delta()
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
        "function_call" | "custom_tool_call" => ("tool_call", item["call_id"].clone(), item["name"].clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::MAX_REQUEST_BODY;
    use bytes::Bytes;

    fn parser() -> CodexParser {
        CodexParser::new(false)
    }

    fn identity_of(headers: &[(&str, &str)]) -> Identity {
        let head = RequestHead {
            method: "POST".into(),
            host: "chatgpt.com".into(),
            path: "/backend-api/codex/responses".into(),
            headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        };
        CodexAdapter::default().identity(&head)
    }

    #[test]
    fn the_session_and_thread_headers_are_read() {
        // It reads both ids and finds no parent or turn
        let identity = identity_of(&[("session-id", "s1"), ("thread-id", "t1")]);

        assert_eq!(
            identity,
            Identity {
                session: Some("s1".into()),
                thread: Some("t1".into()),
                parent_thread: None,
                turn: None,
            }
        );
    }

    #[test]
    fn a_parent_thread_header_gives_the_parent() {
        // It reads the thread that launched this one
        let identity = identity_of(&[("thread-id", "t2"), ("x-codex-parent-thread-id", "t1")]);

        assert_eq!(identity.parent_thread.as_deref(), Some("t1"));
    }

    #[test]
    fn the_turn_comes_from_the_turn_metadata_json() {
        // It reads turn_id out of the header's JSON
        let identity = identity_of(&[("x-codex-turn-metadata", r#"{"turn_id":"u1","session_id":"s9"}"#)]);

        assert_eq!(identity.turn.as_deref(), Some("u1"));
    }

    #[test]
    fn malformed_turn_metadata_gives_no_turn_and_keeps_the_rest() {
        // It still reads session and thread
        let identity = identity_of(&[
            ("session-id", "s1"),
            ("thread-id", "t1"),
            ("x-codex-turn-metadata", "{not json"),
        ]);

        assert_eq!(identity.turn, None);
        assert_eq!(identity.session.as_deref(), Some("s1"));
        assert_eq!(identity.thread.as_deref(), Some("t1"));
    }

    #[test]
    fn no_ids_give_an_empty_identity() {
        // It reads nothing it was not sent
        assert_eq!(identity_of(&[]), Identity::default());
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
    fn maps_roles_other_than_assistant_and_system_ones_to_user() {
        // It keeps messages to user and assistant, folding any other role into user
        let events = parser().request_events(
            br#"{"input":[{"type":"message","role":"tool","content":"d"},{"type":"message","role":"user","content":"u"}]}"#,
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
    fn moves_developer_and_system_items_to_system_and_leaves_messages() {
        // It keeps them in order as text entries and out of messages
        let body = br#"{"input":[
            {"type":"message","role":"developer","content":[{"type":"input_text","text":"d1"}]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
            {"type":"message","role":"system","content":"s1"}]}"#;
        let mut parser = parser();

        parser.feed(TapInput::RequestBody(Bytes::from_static(body)));
        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(
            events[0].payload["system"],
            json!([{"type": "text", "text": "d1"}, {"type": "text", "text": "s1"}])
        );
        assert_eq!(
            events[0].payload["messages"],
            json!([message("user", vec![text_block("hi")])])
        );
    }

    #[test]
    fn reads_tools_from_an_additional_tools_item() {
        // It passes the item's tools through unchanged and keeps it out of messages
        let body = br#"{"input":[{"type":"additional_tools","role":"developer","tools":[{"type":"namespace","name":"n","tools":[]}]}]}"#;
        let mut parser = parser();

        parser.feed(TapInput::RequestBody(Bytes::from_static(body)));
        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(
            events[0].payload["tools"],
            json!([{"type": "namespace", "name": "n", "tools": []}])
        );
        assert_eq!(events[0].payload["messages"], json!([]));
    }

    #[test]
    fn top_level_instructions_and_tools_win_over_the_input_forms() {
        // It ignores developer text and additional tools when the request has the top-level fields
        let body = br#"{"instructions":"top","tools":[{"name":"top"}],"input":[
            {"type":"message","role":"developer","content":"d"},
            {"type":"additional_tools","tools":[{"name":"item"}]}]}"#;
        let mut parser = parser();

        parser.feed(TapInput::RequestBody(Bytes::from_static(body)));
        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(events[0].payload["system"], "top");
        assert_eq!(events[0].payload["tools"], json!([{"name": "top"}]));
    }

    #[test]
    fn a_request_without_system_or_tools_keeps_both_null() {
        // It leaves system and tools null
        let body = br#"{"input":[{"type":"message","role":"user","content":"hi"}]}"#;
        let mut parser = parser();

        parser.feed(TapInput::RequestBody(Bytes::from_static(body)));
        let events = parser.feed(TapInput::RequestEnd);

        assert!(events[0].payload["system"].is_null());
        assert!(events[0].payload["tools"].is_null());
    }

    #[test]
    fn maps_custom_tool_calls_and_their_output_in_a_request() {
        // It keeps the raw tool input string as the call input, and the output as a tool result
        let body = br#"{"input":[
            {"type":"custom_tool_call","call_id":"c1","name":"apply_patch","input":"*** Begin Patch"},
            {"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_text","text":"done"}]}]}"#;
        let mut parser = parser();

        parser.feed(TapInput::RequestBody(Bytes::from_static(body)));
        let events = parser.feed(TapInput::RequestEnd);

        assert_eq!(
            events[0].payload["messages"],
            json!([
                message(
                    "assistant",
                    vec![tool_call_block(
                        json!("c1"),
                        json!("apply_patch"),
                        json!("*** Begin Patch")
                    )]
                ),
                message(
                    "user",
                    vec![tool_result_block(json!("c1"), json!([text_block("done")]), false)]
                ),
            ])
        );
    }

    #[test]
    fn streams_a_custom_tool_call_as_a_tool_call_block() {
        // It starts a tool_call block from the item and passes the input text through as deltas
        let stream = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"custom_tool_call\",\"call_id\":\"c1\",\"name\":\"apply_patch\"}}\n\n",
            "data: {\"type\":\"response.custom_tool_call_input.delta\",\"output_index\":0,\"delta\":\"*** Begin\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"custom_tool_call\"}}\n\n",
        )
        .as_bytes();
        let mut parser = parser();

        let mut events = parser.feed(TapInput::ResponseBody(Bytes::from_static(stream)));
        events.extend(parser.feed(TapInput::ResponseEnd));

        assert_eq!(kinds(&events), ["block_start", "block_delta", "block_stop"]);
        assert_eq!(
            events[0].payload,
            json!({"index": 0, "kind": "tool_call", "id": "c1", "name": "apply_patch"})
        );
        assert_eq!(events[1].payload, json!({"index": 0, "input_json": "*** Begin"}));
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

    fn client(parser: &mut CodexParser, frame: Value) -> Vec<TapEvent> {
        parser.feed(TapInput::WsMessage {
            from_client: true,
            text: frame.to_string(),
            start: 0,
            end: 1,
        })
    }

    fn server(parser: &mut CodexParser, frames: &[Value]) {
        for frame in frames {
            parser.feed(TapInput::WsMessage {
                from_client: false,
                text: frame.to_string(),
                start: 0,
                end: 1,
            });
        }
    }

    fn user_item(text: &str) -> Value {
        json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})
    }

    fn reply_frames(id: &str, text: &str) -> Vec<Value> {
        vec![
            json!({"type": "response.created", "response": {"id": id}}),
            json!({"type": "response.output_item.done", "output_index": 0, "item": {
                "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}}),
            json!({"type": "response.completed", "response": {"id": id, "status": "completed"}}),
        ]
    }

    fn first_request(parser: &mut CodexParser) -> TapEvent {
        client(
            parser,
            json!({"type": "response.create", "input": [
                {"type": "message", "role": "developer", "content": "sys"},
                {"type": "additional_tools", "tools": [{"name": "t"}]},
                user_item("one")]}),
        )
        .remove(0)
    }

    #[test]
    fn a_continuing_request_is_emitted_as_stored_history_reply_and_new_items() {
        // It joins the first request, its reply and the new items, and carries system and tools
        let mut parser = parser();
        first_request(&mut parser);
        server(&mut parser, &reply_frames("r1", "answer"));

        let event = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
        )
        .remove(0);

        assert_eq!(
            event.payload["messages"],
            json!([
                message("user", vec![text_block("one")]),
                message("assistant", vec![text_block("answer")]),
                message("user", vec![text_block("two")]),
            ])
        );
        assert_eq!(event.payload["system"], json!([{"type": "text", "text": "sys"}]));
        assert_eq!(event.payload["tools"], json!([{"name": "t"}]));
        assert!(event.payload["partial"].is_null());
    }

    #[test]
    fn a_request_without_a_previous_id_starts_a_new_history() {
        // It forgets the earlier turns and the earlier system and tools
        let mut parser = parser();
        first_request(&mut parser);
        server(&mut parser, &reply_frames("r1", "answer"));

        let event = client(
            &mut parser,
            json!({"type": "response.create", "input": [user_item("fresh")]}),
        )
        .remove(0);

        assert_eq!(
            event.payload["messages"],
            json!([message("user", vec![text_block("fresh")])])
        );
        assert!(event.payload["system"].is_null());
        assert!(event.payload["tools"].is_null());
    }

    #[test]
    fn a_later_request_that_sets_system_replaces_the_stored_one() {
        let mut parser = parser();
        first_request(&mut parser);
        server(&mut parser, &reply_frames("r1", "answer"));

        let event = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "r1", "instructions": "new", "input": [user_item("two")]}),
        )
        .remove(0);

        assert_eq!(event.payload["system"], json!("new"));
        assert_eq!(event.payload["tools"], json!([{"name": "t"}]));
    }

    #[test]
    fn an_unknown_previous_id_is_partial_and_stays_partial() {
        // It emits the request as sent, then does not chain from it until a new history starts
        let mut parser = parser();
        let event = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "gone", "input": [user_item("two")]}),
        )
        .remove(0);
        assert_eq!(event.payload["partial"], json!(true));
        assert_eq!(
            event.payload["messages"],
            json!([message("user", vec![text_block("two")])])
        );

        server(&mut parser, &reply_frames("r2", "answer"));
        let next = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "r2", "input": [user_item("three")]}),
        )
        .remove(0);
        assert_eq!(next.payload["partial"], json!(true));

        let fresh = client(
            &mut parser,
            json!({"type": "response.create", "input": [user_item("new")]}),
        )
        .remove(0);
        assert!(fresh.payload["partial"].is_null());
    }

    #[test]
    fn a_reply_with_fewer_items_than_its_output_breaks_the_chain() {
        // A dropped output_item.done frame must not give a silently short history
        let mut parser = parser();
        first_request(&mut parser);
        server(
            &mut parser,
            &[
                json!({"type": "response.created", "response": {"id": "r1"}}),
                json!({"type": "response.completed", "response": {"id": "r1", "output": [{"type": "message"}]}}),
            ],
        );

        let event = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
        )
        .remove(0);

        assert_eq!(event.payload["partial"], json!(true));
    }

    #[test]
    fn history_past_the_bound_makes_later_records_partial() {
        let mut parser = parser();
        first_request(&mut parser);
        parser.history.bytes = MAX_REQUEST_BODY;
        server(&mut parser, &reply_frames("r1", "answer"));

        let event = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
        )
        .remove(0);

        assert_eq!(event.payload["partial"], json!(true));
    }

    #[test]
    fn parsers_do_not_share_history() {
        let mut a = parser();
        let mut b = parser();
        first_request(&mut a);
        server(&mut a, &reply_frames("r1", "answer"));

        let event = client(
            &mut b,
            json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
        )
        .remove(0);

        assert_eq!(event.payload["partial"], json!(true));
    }

    #[test]
    fn a_generate_false_request_is_marked_warmup_and_the_chain_runs_through_it() {
        // Only the warm-up carries the flag, and the next request continues from its reply
        let mut parser = parser();
        let warm = client(
            &mut parser,
            json!({"type": "response.create", "generate": false, "input": [user_item("one")]}),
        )
        .remove(0);
        assert_eq!(warm.payload["warmup"], json!(true));
        server(
            &mut parser,
            &[
                json!({"type": "response.created", "response": {"id": "w1"}}),
                json!({"type": "response.completed", "response": {"id": "w1", "output": []}}),
            ],
        );

        let next = client(
            &mut parser,
            json!({"type": "response.create", "previous_response_id": "w1", "input": [user_item("two")]}),
        )
        .remove(0);

        assert!(next.payload["warmup"].is_null());
        assert!(next.payload["partial"].is_null());
        assert_eq!(
            next.payload["messages"],
            json!([message("user", vec![text_block("one"), text_block("two")])])
        );
    }
}
