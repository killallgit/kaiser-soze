use super::*;
use crate::adapter::event::*;
use crate::adapter::{Event, ExchangeParser, TapEvent, TapInput};
use crate::capture::MAX_BUFFER_BYTES;
use crate::common::message::{Block, Message, Output};
use crate::common::*;
use crate::graph::{Identity, ThreadRole};
use bytes::Bytes;
use serde_json::{json, Value};

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
            role: ThreadRole::Unknown,
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
fn a_thread_equal_to_its_session_is_main() {
    // It needs both ids, equal, and no sign of a parent
    let identity = identity_of(&[("session-id", "s1"), ("thread-id", "s1")]);

    assert_eq!(identity.role, ThreadRole::Main);
}

#[test]
fn a_parent_thread_header_makes_a_subagent() {
    // It wins even when the thread equals the session
    let identity = identity_of(&[
        ("session-id", "s1"),
        ("thread-id", "s1"),
        ("x-codex-parent-thread-id", "t0"),
    ]);

    assert_eq!(identity.role, ThreadRole::Subagent);
}

#[test]
fn a_subagent_header_alone_makes_a_subagent() {
    // It reads x-openai-subagent without a parent header
    let identity = identity_of(&[
        ("session-id", "s1"),
        ("thread-id", "s1"),
        ("x-openai-subagent", "collab_spawn"),
    ]);

    assert_eq!(identity.role, ThreadRole::Subagent);
}

#[test]
fn an_empty_subagent_header_does_not_block_main() {
    // An empty value is no evidence either way
    let identity = identity_of(&[("session-id", "s1"), ("thread-id", "s1"), ("x-openai-subagent", "")]);

    assert_eq!(identity.role, ThreadRole::Main);
}

#[test]
fn missing_or_different_ids_are_unknown() {
    // It never promotes a thread it cannot tie to the session
    let different = identity_of(&[("session-id", "s1"), ("thread-id", "t1")]);
    let no_session = identity_of(&[("thread-id", "t1")]);
    let no_thread = identity_of(&[("session-id", "s1")]);

    for identity in [different, no_session, no_thread, identity_of(&[])] {
        assert_eq!(identity.role, ThreadRole::Unknown);
    }
}

#[test]
fn the_turn_comes_from_the_turn_metadata_json() {
    // It reads turn_id out of the header's JSON
    let identity = identity_of(&[("x-codex-turn-metadata", r#"{"turn_id":"u1","session_id":"s9"}"#)]);

    assert_eq!(identity.turn.as_deref(), Some("u1"));
}

#[test]
fn an_empty_turn_id_in_the_turn_metadata_names_no_turn() {
    // The prewarm request sends turn_id as an empty string
    let identity = identity_of(&[("x-codex-turn-metadata", r#"{"turn_id":"","session_id":"s9"}"#)]);

    assert_eq!(identity.turn, None);
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

fn untitled(index: u64, kind: BlockKind) -> BlockStart {
    BlockStart {
        id: Value::Null,
        index,
        kind,
        name: Value::Null,
    }
}

fn kinds(events: &[TapEvent]) -> Vec<&'static str> {
    events.iter().map(|e| e.event.kind()).collect()
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
    assert_eq!(events[0].event.conversation().system, "sys");
    assert_eq!(
        events[0].event.conversation().tools,
        json!([{"type": "function", "name": "shell"}])
    );
    assert_eq!(
        events[0].event.conversation().messages,
        vec![
            Message::user(vec![Block::text("hi")]),
            Message::assistant(vec![
                Block::reasoning(json!("think"), Value::Null),
                Block::ToolCall {
                    id: json!("c1"),
                    input: json!({"cmd": "ls"}),
                    name: json!("shell")
                },
            ]),
            Message::user(vec![
                Block::ToolResult {
                    call_id: json!("c1"),
                    is_error: false,
                    output: Output::Raw(json!("out"))
                },
                Block::Image {
                    content_type: "image/png".into(),
                    reference: "4:0".into(),
                    size: 3
                }
            ]),
        ]
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
        events[0].event.conversation().messages,
        vec![Message::user(vec![Block::text("d"), Block::text("u")])]
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
        events[0].event.conversation().messages,
        vec![Message::user(vec![Block::text("hi")])]
    );
}

#[test]
fn each_request_on_a_socket_is_the_next_step_and_names_its_own_turn() {
    // Two prompts on one socket land in different turns; a request without a turn names none
    let mut parser = parser();
    let mut conversation = |body: Value| {
        let text = body.to_string();
        let events = parser.feed(TapInput::WsMessage {
            from_client: true,
            text,
            start: 0,
            end: 1,
        });
        events[0].event.conversation().clone()
    };

    let first = conversation(json!({"type":"response.create","input":[],"client_metadata":{"turn_id":"u1"}}));
    let second = conversation(json!({"type":"response.create","input":[],"client_metadata":{"turn_id":"u1"}}));
    let third = conversation(json!({"type":"response.create","input":[],"client_metadata":{"turn_id":"u2"}}));
    let bare = conversation(json!({"type":"response.create","input":[]}));

    assert_eq!(
        [first.step, second.step, third.step, bare.step],
        [Some(1), Some(2), Some(3), Some(4)]
    );
    assert_eq!(
        [
            first.turn.as_deref(),
            second.turn.as_deref(),
            third.turn.as_deref(),
            bare.turn.as_deref()
        ],
        [Some("u1"), Some("u1"), Some("u2"), None]
    );
}

#[test]
fn a_request_with_an_empty_turn_id_names_no_turn() {
    // The prewarm request is a step but belongs to no turn
    let mut parser = parser();
    let events = parser.feed(TapInput::WsMessage {
        from_client: true,
        text: json!({"type":"response.create","input":[],"client_metadata":{"turn_id":""}}).to_string(),
        start: 0,
        end: 1,
    });

    assert_eq!(events[0].event.conversation().step, Some(1));
    assert_eq!(events[0].event.conversation().turn, None);
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
        events[0].event.conversation().system,
        json!([{"type": "text", "text": "d1"}, {"type": "text", "text": "s1"}])
    );
    assert_eq!(
        events[0].event.conversation().messages,
        vec![Message::user(vec![Block::text("hi")])]
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
        events[0].event.conversation().tools,
        json!([{"type": "namespace", "name": "n", "tools": []}])
    );
    assert_eq!(events[0].event.conversation().messages, vec![]);
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

    assert_eq!(events[0].event.conversation().system, "top");
    assert_eq!(events[0].event.conversation().tools, json!([{"name": "top"}]));
}

#[test]
fn a_request_without_system_or_tools_keeps_both_null() {
    // It leaves system and tools null
    let body = br#"{"input":[{"type":"message","role":"user","content":"hi"}]}"#;
    let mut parser = parser();

    parser.feed(TapInput::RequestBody(Bytes::from_static(body)));
    let events = parser.feed(TapInput::RequestEnd);

    assert!(events[0].event.conversation().system.is_null());
    assert!(events[0].event.conversation().tools.is_null());
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
        events[0].event.conversation().messages,
        vec![
            Message::assistant(vec![Block::ToolCall {
                id: json!("c1"),
                input: json!("*** Begin Patch"),
                name: json!("apply_patch")
            }]),
            Message::user(vec![Block::ToolResult {
                call_id: json!("c1"),
                is_error: false,
                output: Output::Blocks(vec![Block::text("done")])
            }]),
        ]
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
        events[0].event,
        block_start(BlockStart {
            id: json!("c1"),
            index: 0,
            kind: BlockKind::ToolCall,
            name: json!("apply_patch")
        })
        .event
    );
    assert_eq!(
        events[1].event,
        block_delta(0, Delta::InputJson("*** Begin".into())).event
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
    assert_eq!(events[0].event, block_start(untitled(0, BlockKind::Reasoning)).event);
    assert_eq!(events[1].event, block_delta(0, Delta::Text("hm".into())).event);
    assert_eq!(events[2].event, block_delta(0, Delta::Encrypted("enc".into())).event);
    assert_eq!(events[4].event, block_start(untitled(1, BlockKind::Text)).event);
    assert_eq!(
        events[6].event,
        block_start(BlockStart {
            id: json!("c1"),
            index: 2,
            kind: BlockKind::ToolCall,
            name: json!("shell")
        })
        .event
    );
    assert_eq!(events[7].event, block_delta(2, Delta::InputJson("{}".into())).event);
    assert_eq!(
        events[8].event,
        usage(Usage {
            input: Some(3),
            output: Some(1),
            cache_read: Some(2),
            cache_write: None,
            reasoning: Some(1)
        })
        .event
    );
    assert_eq!(
        events[9].event,
        reply_end_of_message(json!("completed"), json!("r1")).event
    );
}

fn server_message(parser: &mut CodexParser, text: &str) -> Vec<TapEvent> {
    parser.feed(TapInput::WsMessage {
        from_client: false,
        text: text.into(),
        start: 0,
        end: 1,
    })
}

fn reply_end_of(events: &[TapEvent]) -> Event {
    events.iter().map(|e| e.event.clone()).next_back().expect("an event")
}

#[test]
fn the_end_frame_response_id_is_the_reply_message_id() {
    // It prefers the id on the end frame over the one seen at response.created
    let mut parser = parser();
    server_message(&mut parser, r#"{"type":"response.created","response":{"id":"r1"}}"#);

    let events = server_message(
        &mut parser,
        r#"{"type":"response.completed","response":{"id":"r2","status":"completed"}}"#,
    );

    assert_eq!(
        reply_end_of(&events),
        reply_end_of_message(json!("completed"), json!("r2")).event
    );
}

#[test]
fn an_end_frame_without_an_id_uses_the_id_from_response_created() {
    // It falls back to the id remembered for the same reply
    let mut parser = parser();
    server_message(&mut parser, r#"{"type":"response.created","response":{"id":"r1"}}"#);

    let events = server_message(
        &mut parser,
        r#"{"type":"response.incomplete","response":{"status":"incomplete"}}"#,
    );

    assert_eq!(
        reply_end_of(&events),
        reply_end_of_message(json!("incomplete"), json!("r1")).event
    );
}

#[test]
fn a_later_reply_does_not_inherit_the_previous_reply_id() {
    // The first reply's remembered id is cleared when its end frame is handled
    let mut parser = parser();
    server_message(&mut parser, r#"{"type":"response.created","response":{"id":"r1"}}"#);
    server_message(
        &mut parser,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    );

    let events = server_message(
        &mut parser,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    );

    assert_eq!(
        reply_end_of(&events),
        reply_end_of_message(json!("completed"), Value::Null).event
    );
}

#[test]
fn a_failed_reply_does_not_leak_its_id_into_the_next_reply() {
    // The error clears the remembered id
    let mut parser = parser();
    server_message(&mut parser, r#"{"type":"response.created","response":{"id":"r1"}}"#);
    server_message(
        &mut parser,
        r#"{"type":"response.failed","response":{"error":{"code":"x"}}}"#,
    );

    let events = server_message(
        &mut parser,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    );

    assert_eq!(
        reply_end_of(&events),
        reply_end_of_message(json!("completed"), Value::Null).event
    );
}

#[test]
fn a_new_request_clears_the_id_of_a_reply_that_never_ended() {
    // A truncated reply leaves no id behind for the next one
    let mut parser = parser();
    server_message(&mut parser, r#"{"type":"response.created","response":{"id":"r1"}}"#);
    parser.feed(TapInput::WsMessage {
        from_client: true,
        text: r#"{"type":"response.create"}"#.into(),
        start: 0,
        end: 1,
    });

    let events = server_message(
        &mut parser,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    );

    assert_eq!(
        reply_end_of(&events),
        reply_end_of_message(json!("completed"), Value::Null).event
    );
}

#[test]
fn an_output_item_id_is_not_the_reply_message_id() {
    // It ignores ids on output items
    let mut parser = parser();
    server_message(
        &mut parser,
        r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg1"}}"#,
    );

    let events = server_message(
        &mut parser,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    );

    assert_eq!(
        reply_end_of(&events),
        reply_end_of_message(json!("completed"), Value::Null).event
    );
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
    assert_eq!(events[0].event, block_delta(1, Delta::Text("x".into())).event);
}

#[test]
fn reports_a_failure_by_code_only() {
    // It leaves the upstream message text out
    let mut parser = parser();

    let events = parser.feed(TapInput::ResponseBody(Bytes::from_static(
        b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit\",\"message\":\"slow\"}}}\n\n",
    )));

    assert_eq!(events[0].event, error(json!("rate_limit")).event);
}

#[test]
fn flags_a_request_body_over_the_cap_as_unparsed() {
    // It reports the request as unparsed rather than reading a partial body
    let mut parser = parser();
    parser.feed(TapInput::RequestBody(Bytes::from(vec![b' '; MAX_BUFFER_BYTES])));
    parser.feed(TapInput::RequestBody(Bytes::from_static(b"{}")));

    let events = parser.feed(TapInput::RequestEnd);

    assert_eq!(kinds(&events), ["unparsed"]);
    assert_eq!(events[0].event, unparsed(Side::Request).event);
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
    assert_eq!(events[0].event, unparsed(Side::Reply).event);
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
        event.event.conversation().messages,
        vec![
            Message::user(vec![Block::text("one")]),
            Message::assistant(vec![Block::text("answer")]),
            Message::user(vec![Block::text("two")]),
        ]
    );
    assert_eq!(
        event.event.conversation().system,
        json!([{"type": "text", "text": "sys"}])
    );
    assert_eq!(event.event.conversation().tools, json!([{"name": "t"}]));
    assert!(!event.event.conversation().partial);
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
        event.event.conversation().messages,
        vec![Message::user(vec![Block::text("fresh")])]
    );
    assert!(event.event.conversation().system.is_null());
    assert!(event.event.conversation().tools.is_null());
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

    assert_eq!(event.event.conversation().system, json!("new"));
    assert_eq!(event.event.conversation().tools, json!([{"name": "t"}]));
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
    assert!(event.event.conversation().partial);
    assert_eq!(
        event.event.conversation().messages,
        vec![Message::user(vec![Block::text("two")])]
    );

    server(&mut parser, &reply_frames("r2", "answer"));
    let next = client(
        &mut parser,
        json!({"type": "response.create", "previous_response_id": "r2", "input": [user_item("three")]}),
    )
    .remove(0);
    assert!(next.event.conversation().partial);

    let fresh = client(
        &mut parser,
        json!({"type": "response.create", "input": [user_item("new")]}),
    )
    .remove(0);
    assert!(!fresh.event.conversation().partial);
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

    assert!(event.event.conversation().partial);
}

#[test]
fn history_past_the_bound_makes_later_records_partial() {
    let mut parser = parser();
    first_request(&mut parser);
    server(&mut parser, &reply_frames("r1", &"a".repeat(MAX_BUFFER_BYTES)));

    let event = client(
        &mut parser,
        json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
    )
    .remove(0);

    assert!(event.event.conversation().partial);
}

#[test]
fn a_request_body_that_cannot_be_read_makes_the_next_request_partial() {
    let mut parser = parser();
    first_request(&mut parser);
    server(&mut parser, &reply_frames("r1", "answer"));
    parser.feed(TapInput::RequestBody(Bytes::from(vec![b' '; MAX_BUFFER_BYTES])));
    parser.feed(TapInput::RequestBody(Bytes::from_static(b"{}")));
    parser.feed(TapInput::RequestEnd);

    let event = client(
        &mut parser,
        json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
    )
    .remove(0);

    assert!(event.event.conversation().partial);
}

#[test]
fn a_request_message_that_is_not_json_makes_the_next_request_partial() {
    let mut parser = parser();
    first_request(&mut parser);
    server(&mut parser, &reply_frames("r1", "answer"));
    parser.feed(TapInput::WsMessage {
        from_client: true,
        text: "not json".into(),
        start: 0,
        end: 8,
    });

    let event = client(
        &mut parser,
        json!({"type": "response.create", "previous_response_id": "r1", "input": [user_item("two")]}),
    )
    .remove(0);

    assert!(event.event.conversation().partial);
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

    assert!(event.event.conversation().partial);
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
    assert!(warm.event.conversation().warmup);
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

    assert!(!next.event.conversation().warmup);
    assert!(!next.event.conversation().partial);
    assert_eq!(
        next.event.conversation().messages,
        vec![Message::user(vec![Block::text("one"), Block::text("two")])]
    );
}

#[test]
fn a_message_is_sized_as_the_bytes_it_writes() {
    // The size bound counts the written bytes, as it did when messages were plain JSON
    let message = Message::user(vec![Block::text("é\n"), Block::reasoning(Value::Null, json!("s"))]);

    let size = super::history::size_of(std::slice::from_ref(&message));

    assert_eq!(size, serde_json::to_value(&message).unwrap().to_string().len());
}
