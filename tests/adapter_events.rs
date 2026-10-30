use kaiser_soze::adapter::event::*;
use kaiser_soze::common::event::{
    BlockDelta, BlockKind, BlockStart, BlockStop, Conversation, Delta, Error, Event, ReplyEnd, Side, Unparsed,
};
use kaiser_soze::common::message::Block;
use kaiser_soze::common::message::Message;
use serde_json::json;
use serde_json::Value;

#[test]
fn builds_each_live_event_with_its_contract_kind_and_fields() {
    // It names the kinds in contracts/records.md and carries only their fields
    let conversation = conversation(
        json!("sys"),
        json!([{"name": "shell"}]),
        vec![Message::user(vec![Block::text("hi")])],
    );
    let start = block_start(BlockStart {
        id: json!("c1"),
        index: 1,
        kind: BlockKind::ToolCall,
        name: json!("shell"),
    });
    let delta = block_delta(1, Delta::InputJson("{}".into()));
    let stop = block_stop(1);
    let end = reply_end_of_message(json!("end_turn"), Value::Null);
    let failed = error(json!("overloaded_error"));
    let unreadable = unparsed(Side::Reply);

    let kinds: Vec<_> = [&conversation, &start, &delta, &stop, &end, &failed, &unreadable]
        .iter()
        .map(|event| event.event.kind())
        .collect();

    assert_eq!(
        kinds,
        [
            "conversation",
            "block_start",
            "block_delta",
            "block_stop",
            "reply_end",
            "error",
            "unparsed"
        ]
    );
    assert_eq!(
        conversation.event,
        Event::Conversation(Conversation::new(
            json!("sys"),
            json!([{"name": "shell"}]),
            vec![Message::user(vec![Block::text("hi")])]
        ))
    );
    assert_eq!(
        delta.event,
        Event::BlockDelta(BlockDelta {
            index: 1,
            delta: Delta::InputJson("{}".into())
        })
    );
    assert_eq!(stop.event, Event::BlockStop(BlockStop { index: 1 }));
    assert_eq!(
        end.event,
        Event::ReplyEnd(ReplyEnd {
            message_id: None,
            stop_reason: json!("end_turn")
        })
    );
    assert_eq!(
        failed.event,
        Event::Error(Error {
            error_type: json!("overloaded_error")
        })
    );
    assert_eq!(unreadable.event, Event::Unparsed(Unparsed { side: Side::Reply }));
}

#[test]
fn a_reply_end_of_a_message_names_the_message_unless_its_id_is_null() {
    // A null id is no id
    let named = reply_end_of_message(json!("end_turn"), json!("msg_1"));
    let unnamed = reply_end_of_message(json!("end_turn"), Value::Null);

    assert_eq!(
        named.event,
        Event::ReplyEnd(ReplyEnd {
            message_id: Some(json!("msg_1")),
            stop_reason: json!("end_turn")
        })
    );
    assert_eq!(
        unnamed.event,
        Event::ReplyEnd(ReplyEnd {
            message_id: None,
            stop_reason: json!("end_turn")
        })
    );
}

#[test]
fn at_sets_the_byte_span_of_an_event() {
    // It moves the event to the given start and end offsets
    let event = block_stop(0).at(4, 9);

    assert_eq!((event.start, event.end), (4, 9));
}
