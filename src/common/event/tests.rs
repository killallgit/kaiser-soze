use super::*;
use crate::common::message::{Block, Message};
use serde_json::json;

fn written(event: &impl Serialize) -> String {
    serde_json::to_string(event).unwrap()
}

fn bare_conversation() -> Conversation {
    Conversation::new(Value::Null, Value::Null, vec![Message::user(vec![Block::text("hi")])])
}

#[test]
fn a_bare_conversation_writes_null_system_and_tools_and_leaves_out_every_optional_field() {
    // Only the messages, the system and the tools are always written
    assert_eq!(
        written(&bare_conversation()),
        r#"{"messages":[{"blocks":[{"kind":"text","text":"hi"}],"role":"user"}],"system":null,"tools":null}"#
    );
}

#[test]
fn a_conversation_writes_each_field_it_has_in_key_order() {
    // The flags are written only when true
    let conversation = Conversation {
        partial: true,
        previous_message_id: Some("m1".into()),
        step: Some(3),
        turn: Some("u2".into()),
        warmup: true,
        system: json!("sys"),
        tools: json!([{"name": "shell"}]),
        ..Conversation::new(Value::Null, Value::Null, Vec::new())
    };

    assert_eq!(
        written(&conversation),
        concat!(
            r#"{"messages":[],"partial":true,"previous_message_id":"m1","step":3,"#,
            r#""system":"sys","tools":[{"name":"shell"}],"turn":"u2","warmup":true}"#
        )
    );
}

#[test]
fn a_conversation_event_writes_counts_and_ids_instead_of_the_conversation() {
    // The record carries the conversation; the live line only announces it, so a request is not written twice
    let conversation = Conversation {
        partial: true,
        previous_message_id: Some("m1".into()),
        step: Some(3),
        turn: Some("u2".into()),
        warmup: true,
        system: json!("sys"),
        tools: json!([{"name": "shell"}]),
        ..bare_conversation()
    };

    assert_eq!(
        written(&Event::Conversation(conversation)),
        concat!(
            r#"{"message_count":1,"partial":true,"previous_message_id":"m1","step":3,"#,
            r#""tool_count":1,"turn":"u2","warmup":true}"#
        )
    );
    assert_eq!(
        written(&Event::Conversation(bare_conversation())),
        r#"{"message_count":1,"tool_count":0}"#
    );
}

#[test]
fn a_block_start_leaves_out_a_null_id_and_name() {
    // A text block names neither; a tool call names both
    let text = BlockStart {
        id: Value::Null,
        index: 0,
        kind: BlockKind::Text,
        name: Value::Null,
    };
    let call = BlockStart {
        id: json!("c1"),
        index: 1,
        kind: BlockKind::ToolCall,
        name: json!("shell"),
    };

    assert_eq!(written(&text), r#"{"index":0,"kind":"text"}"#);
    assert_eq!(
        written(&call),
        r#"{"id":"c1","index":1,"kind":"tool_call","name":"shell"}"#
    );
}

#[test]
fn a_block_start_names_each_kind_in_snake_case() {
    // Reasoning and other are kinds too
    let kinds = [
        BlockKind::Text,
        BlockKind::Reasoning,
        BlockKind::ToolCall,
        BlockKind::Other,
    ];

    let written: Vec<_> = kinds.iter().map(|kind| serde_json::to_value(kind).unwrap()).collect();

    assert_eq!(
        written,
        [json!("text"), json!("reasoning"), json!("tool_call"), json!("other")]
    );
}

#[test]
fn a_block_delta_writes_its_index_and_the_one_field_it_carries() {
    // The field is named for the kind of delta, and the keys are sorted whichever side of the index it falls
    let deltas = [
        Delta::Text("a".into()),
        Delta::InputJson("{}".into()),
        Delta::Signature("s".into()),
        Delta::Encrypted("e".into()),
    ];

    let written: Vec<_> = deltas
        .into_iter()
        .map(|delta| written(&BlockDelta { index: 2, delta }))
        .collect();

    assert_eq!(
        written,
        [
            r#"{"index":2,"text":"a"}"#,
            r#"{"index":2,"input_json":"{}"}"#,
            r#"{"index":2,"signature":"s"}"#,
            r#"{"encrypted":"e","index":2}"#,
        ]
    );
}

#[test]
fn a_block_stop_writes_its_index() {
    assert_eq!(written(&BlockStop { index: 4 }), r#"{"index":4}"#);
}

#[test]
fn usage_writes_all_five_counts_and_a_missing_one_is_null() {
    // The counts are written in key order
    let usage = Usage {
        input: Some(3),
        output: Some(1),
        ..Usage::default()
    };

    assert_eq!(
        written(&usage),
        r#"{"cache_read":null,"cache_write":null,"input":3,"output":1,"reasoning":null}"#
    );
}

#[test]
fn a_later_usage_overwrites_only_the_counts_it_has() {
    // A missing count keeps the earlier one
    let mut usage = Usage {
        input: Some(5),
        cache_read: Some(2),
        ..Usage::default()
    };

    usage.merge(Usage {
        input: Some(9),
        output: Some(4),
        ..Usage::default()
    });

    assert_eq!(
        usage,
        Usage {
            input: Some(9),
            output: Some(4),
            cache_read: Some(2),
            ..Usage::default()
        }
    );
}

#[test]
fn a_reply_end_leaves_out_a_missing_message_id() {
    // The stop reason is written as sent, null included
    let bare = ReplyEnd {
        message_id: None,
        stop_reason: Value::Null,
    };
    let full = ReplyEnd {
        message_id: Some(json!("m1")),
        stop_reason: json!("end_turn"),
    };

    assert_eq!(written(&bare), r#"{"stop_reason":null}"#);
    assert_eq!(written(&full), r#"{"message_id":"m1","stop_reason":"end_turn"}"#);
}

#[test]
fn an_error_and_an_unparsed_side_write_one_field_each() {
    let error = Error {
        error_type: json!("overloaded_error"),
    };
    let request = Unparsed { side: Side::Request };
    let reply = Unparsed { side: Side::Reply };

    assert_eq!(written(&error), r#"{"error_type":"overloaded_error"}"#);
    assert_eq!(written(&request), r#"{"side":"request"}"#);
    assert_eq!(written(&reply), r#"{"side":"reply"}"#);
}

#[test]
fn an_event_writes_its_payload_alone_and_names_its_kind() {
    // The event name is the tap line's own key, so it is not in the payload
    let events = [
        Event::Conversation(bare_conversation()),
        Event::BlockStart(BlockStart {
            id: Value::Null,
            index: 0,
            kind: BlockKind::Other,
            name: Value::Null,
        }),
        Event::BlockDelta(BlockDelta {
            index: 0,
            delta: Delta::Text("a".into()),
        }),
        Event::BlockStop(BlockStop { index: 0 }),
        Event::Usage(Usage::default()),
        Event::ReplyEnd(ReplyEnd {
            message_id: None,
            stop_reason: Value::Null,
        }),
        Event::Error(Error {
            error_type: Value::Null,
        }),
        Event::Unparsed(Unparsed { side: Side::Reply }),
    ];

    let kinds: Vec<_> = events.iter().map(Event::kind).collect();

    assert_eq!(
        kinds,
        [
            "conversation",
            "block_start",
            "block_delta",
            "block_stop",
            "usage",
            "reply_end",
            "error",
            "unparsed"
        ]
    );
    assert_eq!(written(&events[3]), r#"{"index":0}"#);
    assert_eq!(written(&events[7]), r#"{"side":"reply"}"#);
}

#[test]
fn each_payload_reads_back_as_it_was_written() {
    // A tap line can be replayed into the event it was written from
    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
        let read: T = serde_json::from_str(&written(&value)).unwrap();

        assert_eq!(read, value);
    }

    round_trip(bare_conversation());
    round_trip(Conversation {
        partial: true,
        previous_message_id: Some("m1".into()),
        step: Some(3),
        turn: Some("u2".into()),
        warmup: true,
        ..bare_conversation()
    });
    round_trip(BlockStart {
        id: json!("c1"),
        index: 1,
        kind: BlockKind::ToolCall,
        name: json!("shell"),
    });
    round_trip(BlockStart {
        id: Value::Null,
        index: 0,
        kind: BlockKind::Other,
        name: Value::Null,
    });
    round_trip(BlockDelta {
        index: 2,
        delta: Delta::Encrypted("e".into()),
    });
    round_trip(BlockStop { index: 4 });
    round_trip(Usage {
        input: Some(3),
        ..Usage::default()
    });
    round_trip(ReplyEnd {
        message_id: Some(json!("m1")),
        stop_reason: Value::Null,
    });
    round_trip(ReplyEnd {
        message_id: None,
        stop_reason: json!("end_turn"),
    });
    round_trip(Error {
        error_type: json!("overloaded_error"),
    });
    round_trip(Unparsed { side: Side::Request });
}
