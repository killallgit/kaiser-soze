use super::*;
use crate::adapter::{ExchangeParser, TapEvent, TapInput};
use crate::body::MAX_REQUEST_BODY;
use crate::common::*;
use crate::graph::Identity;
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
        events[0].payload.clone()
    };

    let first = conversation(json!({"type":"response.create","input":[],"client_metadata":{"turn_id":"u1"}}));
    let second = conversation(json!({"type":"response.create","input":[],"client_metadata":{"turn_id":"u1"}}));
    let third = conversation(json!({"type":"response.create","input":[],"client_metadata":{"turn_id":"u2"}}));
    let bare = conversation(json!({"type":"response.create","input":[]}));

    assert_eq!(
        [&first["step"], &second["step"], &third["step"]],
        [&json!(1), &json!(2), &json!(3)]
    );
    assert_eq!(
        [&first["turn"], &second["turn"], &third["turn"]],
        [&json!("u1"), &json!("u1"), &json!("u2")]
    );
    assert_eq!(bare["step"], 4);
    assert!(bare.get("turn").is_none());
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

    assert_eq!(events[0].payload["step"], 1);
    assert!(events[0].payload.get("turn").is_none());
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
