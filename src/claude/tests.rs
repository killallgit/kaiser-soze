use super::*;
use crate::adapter::ResponseHead;
use crate::adapter::{ExchangeParser, TapEvent, TapInput};
use crate::message::{Block, Message, Output};
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

fn head_with(headers: &[(&str, &str)]) -> RequestHead {
    RequestHead {
        headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        ..head("/v1/messages")
    }
}

fn identity_of(headers: &[(&str, &str)]) -> Identity {
    ClaudeAdapter::default().identity(&head_with(headers))
}

fn ids(session: &str, thread: &str, parent: Option<&str>) -> Identity {
    Identity {
        session: Some(session.into()),
        thread: Some(thread.into()),
        parent_thread: parent.map(Into::into),
        turn: None,
    }
}

#[test]
fn the_main_agent_is_the_session_thread_with_no_parent() {
    // It uses the session id as the thread when no agent id is sent
    let identity = identity_of(&[
        ("x-claude-code-session-id", "s1"),
        ("x-claude-code-request-class", "main"),
    ]);

    assert_eq!(identity, ids("s1", "s1", None));
}

#[test]
fn a_subagent_is_its_own_thread_under_the_main_agent() {
    // It names the agent as the thread and the session as its parent
    let identity = identity_of(&[
        ("x-claude-code-session-id", "s1"),
        ("x-claude-code-agent-id", "a1"),
        ("x-claude-code-request-class", "subagent"),
    ]);

    assert_eq!(identity, ids("s1", "a1", Some("s1")));
}

#[test]
fn a_subagent_request_without_an_agent_id_has_no_parent() {
    // It would otherwise be a thread spawned by itself
    let identity = identity_of(&[
        ("x-claude-code-session-id", "s1"),
        ("x-claude-code-request-class", "subagent"),
    ]);

    assert_eq!(identity, ids("s1", "s1", None));
}

#[test]
fn a_nested_subagent_names_the_agent_that_launched_it() {
    // It takes the parent agent id over the main agent
    let identity = identity_of(&[
        ("x-claude-code-session-id", "s1"),
        ("x-claude-code-agent-id", "a2"),
        ("x-claude-code-parent-agent-id", "a1"),
        ("x-claude-code-request-class", "subagent"),
    ]);

    assert_eq!(identity, ids("s1", "a2", Some("a1")));
}

#[test]
fn the_prompt_id_is_the_turn() {
    // It reads the turn from the prompt id
    let identity = identity_of(&[("x-claude-code-session-id", "s1"), ("x-claude-code-prompt-id", "p1")]);

    assert_eq!(identity.turn.as_deref(), Some("p1"));
}

#[test]
fn no_ids_give_an_empty_identity() {
    // It reads nothing it was not sent
    assert_eq!(
        identity_of(&[("x-claude-code-request-class", "subagent")]),
        Identity::default()
    );
    assert_eq!(identity_of(&[]), Identity::default());
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
            Message::user(vec![Block::text("hi")]),
            Message::assistant(vec![
                Block::reasoning(json!("hmm"), json!("sig")),
                Block::text("ok"),
                Block::ToolCall {
                    id: json!("t1"),
                    input: json!({"command": "ls"}),
                    name: json!("Bash")
                },
            ]),
            Message::user(vec![
                Block::ToolResult {
                    call_id: json!("t1"),
                    is_error: true,
                    output: Output::Raw(json!("out"))
                },
                Block::Image {
                    content_type: "image/png".into(),
                    reference: "2:1".into(),
                    size: 3
                }
            ]),
        ])
    );
}

#[test]
fn the_previous_message_id_in_the_request_diagnostics_rides_on_the_conversation() {
    // It carries diagnostics.previous_message_id and leaves it out when the body has none
    let with_id = br#"{"messages":[],"diagnostics":{"previous_message_id":"msg_0"}}"#;
    let without_id = br#"{"messages":[],"diagnostics":{}}"#;

    let linked = request(with_id, 64);
    let unlinked = request(without_id, 64);

    assert_eq!(linked[0].payload["previous_message_id"], "msg_0");
    assert!(unlinked[0].payload.get("previous_message_id").is_none());
}

#[test]
fn an_image_inside_a_tool_result_becomes_a_reference() {
    // It never copies the bytes of a nested image
    let body = br#"{"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"see"},{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"QUJDREVGRw=="}}]}]}]}"#;

    let events = request(body, 64);

    let output = &events[0].payload["messages"][0]["blocks"][0]["output"];
    assert_eq!(
        *output,
        json!([
            Block::text("see"),
            Block::Image {
                content_type: "image/jpeg".into(),
                reference: "0:0:1".into(),
                size: 7
            }
        ])
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
    assert_eq!(
        events[5].payload,
        json!({"stop_reason": "end_turn", "message_id": "msg_1"})
    );
}

#[test]
fn a_reply_without_a_message_start_ends_with_no_message_id() {
    // It leaves message_id out of reply_end when the stream never named the message
    let wire = sse(&[
        (
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ),
        ("message_stop", r#"{"type":"message_stop"}"#),
    ]);

    let events = stream(&wire, 64);

    assert_eq!(events[1].payload, json!({"stop_reason": "end_turn"}));
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
