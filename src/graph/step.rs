use super::Identity;
use crate::event::Conversation;
use crate::message::{Block, Message};
use crate::record::Record;
use serde_json::Value;

/// One request and its reply: the unit `turn` and `continues` link.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Step {
    pub(super) thread: Option<String>,
    pub(super) exchange: u64,
    pub(super) n: u64,
    pub(super) turn: Option<String>,
    pub(super) previous_message_id: Option<String>,
    pub(super) message_id: Option<String>,
    /// Call ids of the `tool_result` blocks in the request.
    pub(super) results: Vec<String>,
    /// Ids of the `tool_call` blocks in the reply.
    pub(super) calls: Vec<String>,
}

impl Step {
    /// The step a finished record describes, or none when the record has no request. A first step
    /// that names no turn is in the header turn; a later one may be in a new turn, so it has none.
    pub(super) fn of_record(record: &Record, identity: &Identity, exchange: u64) -> Option<Step> {
        let conversation = record.conversation.as_ref()?;
        let n = claimed_step(conversation);
        let text = |value: &Value| value.as_str().map(String::from);
        let header_turn = (n == 1).then(|| identity.turn.clone()).flatten();
        Some(Step {
            thread: identity.thread.clone(),
            exchange,
            n,
            turn: conversation.turn.clone().or(header_turn),
            previous_message_id: conversation.previous_message_id.clone(),
            message_id: record.reply.message_id.as_ref().and_then(text),
            results: result_ids(conversation),
            calls: call_ids(&record.reply.message),
        })
    }
}

/// The step a request states. A Claude exchange is one request, so it is step 1.
pub(super) fn claimed_step(conversation: &Conversation) -> u64 {
    conversation.step.unwrap_or(1)
}

/// The call ids of the `tool_result` blocks in a request.
fn result_ids(conversation: &Conversation) -> Vec<String> {
    conversation
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            Block::ToolResult { call_id, .. } => call_id.as_str().map(String::from),
            _ => None,
        })
        .collect()
}

/// The ids of the `tool_call` blocks in a reply.
fn call_ids(reply: &Message) -> Vec<String> {
    reply
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::ToolCall { id, .. } => id.as_str().map(String::from),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Output;
    use serde_json::json;

    fn tool_result(call_id: &str) -> Block {
        Block::ToolResult {
            call_id: json!(call_id),
            is_error: false,
            output: Output::Raw(Value::Null),
        }
    }

    fn step_record(step: u64, turn: Option<&str>) -> Record {
        let conversation = Conversation {
            step: Some(step),
            turn: turn.map(Into::into),
            ..Conversation::new(Value::Null, Value::Null, Vec::new())
        };
        Record {
            conversation: Some(conversation),
            ..Record::empty()
        }
    }

    fn identity(turn: Option<&str>) -> Identity {
        Identity {
            thread: Some("t1".into()),
            turn: turn.map(Into::into),
            ..Identity::default()
        }
    }

    #[test]
    fn a_first_step_that_names_no_turn_takes_the_header_turn() {
        // The header turn is the first request's
        let record = step_record(1, None);

        let step = Step::of_record(&record, &identity(Some("h1")), 7).unwrap();

        assert_eq!(step.turn.as_deref(), Some("h1"));
        assert_eq!((step.thread.as_deref(), step.exchange, step.n), (Some("t1"), 7, 1));
    }

    #[test]
    fn an_explicit_turn_beats_the_header_turn() {
        // A Codex step names its own turn, whichever step it is
        let first = Step::of_record(&step_record(1, Some("u2")), &identity(Some("h1")), 7).unwrap();
        let later = Step::of_record(&step_record(3, Some("u2")), &identity(Some("h1")), 7).unwrap();

        assert_eq!(first.turn.as_deref(), Some("u2"));
        assert_eq!(later.turn.as_deref(), Some("u2"));
    }

    #[test]
    fn a_later_step_that_names_no_turn_gets_none() {
        // The header turn does not carry past the first step
        let step = Step::of_record(&step_record(2, None), &identity(Some("h1")), 7).unwrap();

        assert_eq!(step.turn, None);
    }

    #[test]
    fn a_record_without_a_request_gives_no_step() {
        // A reply with no request in front of it cannot be linked
        assert_eq!(Step::of_record(&Record::empty(), &identity(Some("h1")), 7), None);
    }

    #[test]
    fn the_message_ids_and_the_call_ids_are_read_from_the_record() {
        // It reads the previous and reply message ids, the result ids of the request and the call ids of the reply
        let mut conversation = Conversation::new(
            Value::Null,
            Value::Null,
            vec![
                Message::user(vec![tool_result("c1"), Block::text("x")]),
                Message::user(vec![tool_result("c2")]),
            ],
        );
        conversation.previous_message_id = Some("m1".into());
        let mut record = step_record(2, None);
        record.conversation = Some(conversation);
        record.reply.message_id = Some(json!("m2"));
        record.reply.message = Message::assistant(vec![
            Block::ToolCall {
                id: json!("c3"),
                input: Value::Null,
                name: Value::Null,
            },
            Block::text("y"),
        ]);

        let step = Step::of_record(&record, &identity(None), 7).unwrap();

        assert_eq!(step.previous_message_id.as_deref(), Some("m1"));
        assert_eq!(step.message_id.as_deref(), Some("m2"));
        assert_eq!(step.results, ["c1", "c2"]);
        assert_eq!(step.calls, ["c3"]);
    }

    #[test]
    fn a_record_with_none_of_the_ids_gives_a_bare_step() {
        // Missing ids are absent, not empty
        let step = Step::of_record(&step_record(1, None), &Identity::default(), 7).unwrap();

        assert_eq!(
            step,
            Step {
                thread: None,
                exchange: 7,
                n: 1,
                turn: None,
                previous_message_id: None,
                message_id: None,
                results: Vec::new(),
                calls: Vec::new(),
            }
        );
    }
}
