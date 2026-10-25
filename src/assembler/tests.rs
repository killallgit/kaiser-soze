use super::*;
use crate::common::*;
use serde_json::json;

fn conversation_event() -> TapEvent {
    conversation(json!("sys"), json!([]), vec![message("user", vec![text_block("hi")])])
}

fn text_reply(text: &str) -> Vec<TapEvent> {
    vec![
        block_start(BlockStart {
            index: 0,
            kind: "text",
            id: Value::Null,
            name: Value::Null,
        }),
        block_delta(0, Delta::Text(text.into())),
        block_stop(0),
        usage(Usage {
            input: Some(3),
            output: Some(1),
            ..Usage::default()
        }),
        reply_end(json!("end_turn")),
    ]
}

fn feed(assembler: &mut Assembler, events: Vec<TapEvent>, dropped: u64) -> Vec<Value> {
    events.iter().flat_map(|event| assembler.push(event, dropped)).collect()
}

#[test]
fn a_complete_exchange_yields_one_complete_record() {
    // It folds the events into conversation, reply, usage and stop reason
    let mut assembler = Assembler::default();
    let mut events = vec![conversation_event()];
    events.extend(text_reply("ok"));

    let records = feed(&mut assembler, events, 0);

    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["incomplete"], false);
    assert_eq!(record["dropped"], 0);
    assert_eq!(record["conversation"]["system"], "sys");
    assert_eq!(record["reply"]["message"], message("assistant", vec![text_block("ok")]));
    assert_eq!(record["reply"]["usage"]["input"], 3);
    assert_eq!(record["reply"]["usage"]["output"], 1);
    assert_eq!(record["reply"]["stop_reason"], "end_turn");
    assert!(assembler.finish(0).is_none());
}

#[test]
fn dropped_events_mark_the_record_incomplete_and_carry_the_count() {
    // It reports the count it was given
    let mut assembler = Assembler::default();
    let mut events = vec![conversation_event()];
    events.extend(text_reply("ok"));

    let records = feed(&mut assembler, events, 4);

    assert_eq!(records[0]["incomplete"], true);
    assert_eq!(records[0]["dropped"], 4);
}

#[test]
fn finishing_with_an_open_reply_writes_one_incomplete_record() {
    // It keeps what was assembled when the stream is cut off
    let mut assembler = Assembler::default();
    feed(&mut assembler, vec![conversation_event()], 0);
    feed(&mut assembler, text_reply("ok").into_iter().take(2).collect(), 0);

    let record = assembler.finish(0).unwrap();

    assert_eq!(record["incomplete"], true);
    assert_eq!(record["reply"]["message"], message("assistant", vec![text_block("ok")]));
    assert!(assembler.finish(0).is_none());
}

#[test]
fn an_unparsed_request_or_reply_marks_the_record_incomplete() {
    // It treats both sides the same way
    for side in [Side::Request, Side::Reply] {
        let mut assembler = Assembler::default();
        let mut events = vec![unparsed(side)];
        events.extend(text_reply("ok"));

        let records = feed(&mut assembler, events, 0);

        assert_eq!(records[0]["incomplete"], true);
    }
}

#[test]
fn two_replies_on_one_exchange_yield_two_records() {
    // It resets between replies, as a WebSocket does
    let mut assembler = Assembler::default();
    let mut events = vec![conversation_event()];
    events.extend(text_reply("one"));
    events.push(conversation_event());
    events.extend(text_reply("two"));

    let records = feed(&mut assembler, events, 0);

    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "one");
    assert_eq!(records[1]["reply"]["message"]["blocks"][0]["text"], "two");
}

#[test]
fn an_item_outside_the_format_is_skipped_and_later_blocks_keep_their_order() {
    // It leaves the item out without marking the record incomplete
    let mut assembler = Assembler::default();
    let events = vec![
        conversation_event(),
        block_start(BlockStart {
            index: 0,
            kind: "web_search",
            id: Value::Null,
            name: Value::Null,
        }),
        block_delta(0, Delta::Text("skipped".into())),
        block_start(BlockStart {
            index: 1,
            kind: "text",
            id: Value::Null,
            name: Value::Null,
        }),
        block_delta(1, Delta::Text("kept".into())),
        reply_end(json!("end_turn")),
    ];

    let records = feed(&mut assembler, events, 0);

    assert_eq!(records[0]["reply"]["message"]["blocks"], json!([text_block("kept")]));
    assert_eq!(records[0]["incomplete"], false);
}

#[test]
fn a_tool_call_joins_its_json_fragments_into_the_input() {
    // It parses the joined fragments
    let mut assembler = Assembler::default();
    let events = vec![
        conversation_event(),
        block_start(BlockStart {
            index: 0,
            kind: "tool_call",
            id: json!("c1"),
            name: json!("shell"),
        }),
        block_delta(0, Delta::InputJson("{\"a\"".into())),
        block_delta(0, Delta::InputJson(":1}".into())),
        reply_end(json!("tool_use")),
    ];

    let records = feed(&mut assembler, events, 0);

    assert_eq!(
        records[0]["reply"]["message"]["blocks"],
        json!([tool_call_block(json!("c1"), json!("shell"), json!({"a": 1}))])
    );
}

#[test]
fn a_reply_past_the_bound_stops_assembling_and_is_incomplete() {
    // It keeps the part assembled before the bound
    let mut assembler = Assembler::with_bound(5);
    let events = vec![
        conversation_event(),
        block_start(BlockStart {
            index: 0,
            kind: "text",
            id: Value::Null,
            name: Value::Null,
        }),
        block_delta(0, Delta::Text("abc".into())),
        block_delta(0, Delta::Text("defgh".into())),
        reply_end(json!("end_turn")),
    ];

    let records = feed(&mut assembler, events, 0);

    assert_eq!(records[0]["reply"]["message"]["blocks"], json!([text_block("abc")]));
    assert_eq!(records[0]["incomplete"], true);
}

#[test]
fn events_outside_the_common_kinds_write_no_record() {
    // It ignores them, so an unconverted adapter is unaffected
    let mut assembler = Assembler::default();
    let old = TapEvent {
        kind: "message_stop".into(),
        payload: json!({}),
        start: 0,
        end: 0,
    };

    let records = feed(&mut assembler, vec![old], 0);

    assert!(records.is_empty());
    assert!(assembler.finish(0).is_none());
}

#[test]
fn encrypted_reasoning_without_text_becomes_an_encrypted_block() {
    // It keeps the encrypted text when there is no readable text
    let mut assembler = Assembler::default();
    let events = vec![
        block_start(BlockStart {
            index: 0,
            kind: "reasoning",
            id: Value::Null,
            name: Value::Null,
        }),
        block_delta(0, Delta::Encrypted("enc".into())),
        reply_end(json!("completed")),
    ];

    let records = feed(&mut assembler, events, 0);

    assert_eq!(
        records[0]["reply"]["message"]["blocks"],
        json!([encrypted_reasoning_block(json!("enc"))])
    );
}

#[test]
fn usage_fields_merge_across_events() {
    // A later count replaces an earlier one and a missing count keeps the earlier one
    let mut assembler = Assembler::default();
    let events = vec![
        conversation_event(),
        usage(Usage {
            input: Some(9),
            output: Some(1),
            ..Usage::default()
        }),
        usage(Usage {
            output: Some(4),
            ..Usage::default()
        }),
        reply_end(json!("end_turn")),
    ];

    let records = feed(&mut assembler, events, 0);

    assert_eq!(records[0]["reply"]["usage"]["input"], 9);
    assert_eq!(records[0]["reply"]["usage"]["output"], 4);
}

#[test]
fn the_reply_message_id_reaches_the_record_when_the_end_names_one() {
    // It folds reply_end's message_id into the reply and leaves it out otherwise
    let mut assembler = Assembler::default();
    let named = vec![
        conversation_event(),
        reply_end_of_message(json!("end_turn"), json!("msg_1")),
    ];
    let unnamed = vec![conversation_event(), reply_end(json!("end_turn"))];

    let named_records = feed(&mut assembler, named, 0);
    let unnamed_records = feed(&mut assembler, unnamed, 0);

    assert_eq!(named_records[0]["reply"]["message_id"], "msg_1");
    assert!(unnamed_records[0]["reply"].get("message_id").is_none());
}

#[test]
fn a_record_without_a_request_names_no_step() {
    // An unparsed request leaves no step, so it cannot collide with the real first one
    let mut assembler = Assembler::default();
    let events = vec![unparsed(Side::Request), reply_end(json!("end_turn"))];

    let records = feed(&mut assembler, events, 0);

    assert!(records[0].get("step").is_none());
}

#[test]
fn a_request_that_states_its_step_and_turn_gives_them_to_the_record() {
    // It copies the step and turn of the conversation onto the record
    let mut assembler = Assembler::default();
    let mut request = conversation_event();
    request.payload["step"] = json!(3);
    request.payload["turn"] = json!("u2");

    let records = feed(&mut assembler, vec![request, reply_end(json!("end_turn"))], 0);

    assert_eq!(
        (records[0]["step"].clone(), records[0]["turn"].clone()),
        (json!(3), json!("u2"))
    );
}

#[test]
fn a_request_that_states_no_step_is_step_one_and_has_no_turn() {
    // A Claude exchange is one request, so the record is step 1 with no turn of its own
    let mut assembler = Assembler::default();

    let records = feed(
        &mut assembler,
        vec![conversation_event(), reply_end(json!("end_turn"))],
        0,
    );

    assert_eq!(records[0]["step"], 1);
    assert!(records[0].get("turn").is_none());
}

#[test]
fn a_record_without_a_request_names_no_turn() {
    // An unparsed request states neither a step nor a turn
    let mut assembler = Assembler::default();

    let records = feed(
        &mut assembler,
        vec![unparsed(Side::Request), reply_end(json!("end_turn"))],
        0,
    );

    assert!(records[0].get("turn").is_none());
}
