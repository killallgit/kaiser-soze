use super::*;
use serde_json::json;

fn written(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap()
}

#[test]
fn a_text_block_writes_its_kind_then_its_text() {
    // It writes the keys sorted
    assert_eq!(written(&Block::text("hi")), r#"{"kind":"text","text":"hi"}"#);
}

#[test]
fn a_reasoning_block_writes_a_null_text_and_leaves_out_a_missing_signature() {
    // A missing signature is absent and a missing text is null
    let bare = Block::reasoning(Value::Null, Value::Null);
    let signed = Block::reasoning(json!("t"), json!("s"));

    assert_eq!(written(&bare), r#"{"kind":"reasoning","text":null}"#);
    assert_eq!(written(&signed), r#"{"kind":"reasoning","signature":"s","text":"t"}"#);
}

#[test]
fn an_encrypted_reasoning_block_writes_the_encrypted_text_before_its_kind() {
    // The encrypted value may be null
    let sealed = Block::EncryptedReasoning { encrypted: json!("x") };
    let empty = Block::EncryptedReasoning { encrypted: Value::Null };

    assert_eq!(written(&sealed), r#"{"encrypted":"x","kind":"reasoning"}"#);
    assert_eq!(written(&empty), r#"{"encrypted":null,"kind":"reasoning"}"#);
}

#[test]
fn a_tool_call_block_writes_a_null_for_each_missing_field() {
    // Id, name and input each pass through as sent
    let call = Block::ToolCall {
        id: json!("c1"),
        input: json!({"a": 1}),
        name: json!("shell"),
    };
    let missing = Block::ToolCall {
        id: Value::Null,
        input: Value::Null,
        name: Value::Null,
    };

    assert_eq!(
        written(&call),
        r#"{"id":"c1","input":{"a":1},"kind":"tool_call","name":"shell"}"#
    );
    assert_eq!(
        written(&missing),
        r#"{"id":null,"input":null,"kind":"tool_call","name":null}"#
    );
}

#[test]
fn a_tool_result_block_writes_raw_output_as_sent_and_nested_blocks_as_blocks() {
    // The output is either the upstream value or a list of blocks
    let raw = Block::ToolResult {
        call_id: json!("c1"),
        is_error: true,
        output: Output::Raw(json!("ok")),
    };
    let nested = Block::ToolResult {
        call_id: json!("c1"),
        is_error: false,
        output: Output::Blocks(vec![Block::text("a")]),
    };

    assert_eq!(
        written(&raw),
        r#"{"call_id":"c1","is_error":true,"kind":"tool_result","output":"ok"}"#
    );
    assert_eq!(
        written(&nested),
        r#"{"call_id":"c1","is_error":false,"kind":"tool_result","output":[{"kind":"text","text":"a"}]}"#
    );
}

#[test]
fn an_image_block_writes_its_reference_under_ref() {
    // It holds only a reference, a content type and a size
    let image = Block::Image {
        content_type: "image/png".into(),
        reference: "0:2".into(),
        size: 2048,
    };

    assert_eq!(
        written(&image),
        r#"{"content_type":"image/png","kind":"image","ref":"0:2","size":2048}"#
    );
}

#[test]
fn a_message_writes_its_blocks_before_its_role_and_keeps_an_unknown_role() {
    // Any upstream role string passes through
    let message = Message {
        blocks: vec![Block::text("hi")],
        role: Role::from("tool"),
    };

    assert_eq!(
        written(&message),
        r#"{"blocks":[{"kind":"text","text":"hi"}],"role":"tool"}"#
    );
    assert_eq!(written(&Role::from("assistant")), r#""assistant""#);
}

#[test]
fn a_tool_result_output_writes_whatever_the_upstream_sent() {
    // A missing, empty, numeric or object output is not turned into blocks
    let result = |output| {
        written(&Block::ToolResult {
            call_id: json!("c1"),
            is_error: false,
            output,
        })
    };
    let head = r#"{"call_id":"c1","is_error":false,"kind":"tool_result","output":"#;

    assert_eq!(result(Output::Raw(Value::Null)), format!("{head}null}}"));
    assert_eq!(result(Output::Raw(json!({"a": 1}))), format!(r#"{head}{{"a":1}}}}"#));
    assert_eq!(result(Output::Raw(json!(7))), format!("{head}7}}"));
    assert_eq!(result(Output::Blocks(Vec::new())), format!("{head}[]}}"));
}

#[test]
fn a_tool_result_can_hold_an_image() {
    // The nested block keeps its own sorted keys
    let nested = Block::ToolResult {
        call_id: json!("c1"),
        is_error: false,
        output: Output::Blocks(vec![Block::Image {
            content_type: "image/png".into(),
            reference: "0:2".into(),
            size: 1,
        }]),
    };

    assert_eq!(
        written(&nested),
        r#"{"call_id":"c1","is_error":false,"kind":"tool_result","output":[{"content_type":"image/png","kind":"image","ref":"0:2","size":1}]}"#
    );
}

#[test]
fn a_reasoning_signature_that_is_not_a_string_is_written_as_sent() {
    // Only a null signature is left out
    let block = Block::reasoning(json!("t"), json!({"a": 1}));

    assert_eq!(
        written(&block),
        r#"{"kind":"reasoning","signature":{"a":1},"text":"t"}"#
    );
}

#[test]
fn text_is_escaped_as_serde_json_escapes_it_and_a_large_size_is_written_whole() {
    // Unicode stays as it is, control characters and quotes are escaped, and a size past u32 is not cut
    let image = Block::Image {
        content_type: "image/png".into(),
        reference: "0:2".into(),
        size: 5_000_000_000,
    };

    assert_eq!(written(&Block::text("é\n\"")), r#"{"kind":"text","text":"é\n\""}"#);
    assert!(written(&image).ends_with(r#""size":5000000000}"#));
}
