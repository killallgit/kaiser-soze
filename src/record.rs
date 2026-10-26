use crate::event::{Conversation, Usage};
use crate::message::Message;
use serde::Serialize;
use serde_json::Value;

/// One finished request and its reply, as the tap writes it. The fields are declared in the key
/// order of the tap file; an absent id is left out, and a request that was never read is `null`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Record {
    pub conversation: Option<Conversation>,
    pub dropped: u64,
    pub incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub reply: Reply,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
}

/// What the assistant answered. The stop reason passes through as sent; a missing message id is
/// left out, and a missing count is `null`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reply {
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Value>,
    pub stop_reason: Value,
    pub usage: Usage,
}

#[cfg(test)]
impl Record {
    /// A record of an empty reply to no request, with no ids.
    pub(crate) fn empty() -> Self {
        Self {
            conversation: None,
            dropped: 0,
            incomplete: false,
            project: None,
            reply: Reply {
                message: Message::assistant(Vec::new()),
                message_id: None,
                stop_reason: Value::Null,
                usage: Usage::default(),
            },
            session: None,
            step: None,
            thread: None,
            turn: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Block;
    use serde_json::json;

    fn reply() -> Reply {
        Reply {
            message: Message::assistant(vec![Block::text("ok")]),
            message_id: None,
            stop_reason: Value::Null,
            usage: Usage::default(),
        }
    }

    #[test]
    fn a_record_with_no_request_and_no_ids_writes_a_null_conversation_and_leaves_the_ids_out() {
        // It writes null for the conversation and the counts, and nothing for step, ids and message id
        let record = Record {
            reply: reply(),
            ..Record::empty()
        };

        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"conversation":null,"dropped":0,"incomplete":false,"reply":{"message":{"blocks":[{"kind":"text","text":"ok"}],"role":"assistant"},"stop_reason":null,"usage":{"cache_read":null,"cache_write":null,"input":null,"output":null,"reasoning":null}}}"#
        );
    }

    #[test]
    fn a_record_with_every_field_writes_its_keys_in_sorted_order() {
        // It sorts the record, the reply, the conversation and the usage alike
        let record = Record {
            conversation: Some(Conversation {
                step: Some(2),
                turn: Some("u1".into()),
                ..Conversation::new(json!("sys"), json!([]), vec![Message::user(vec![Block::text("hi")])])
            }),
            dropped: 3,
            incomplete: true,
            project: Some("p".into()),
            reply: Reply {
                message_id: Some(json!("m1")),
                stop_reason: json!("end_turn"),
                usage: Usage {
                    input: Some(5),
                    output: Some(1),
                    ..Usage::default()
                },
                ..reply()
            },
            session: Some("s".into()),
            step: Some(2),
            thread: Some("t".into()),
            turn: Some("u1".into()),
        };

        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"conversation":{"messages":[{"blocks":[{"kind":"text","text":"hi"}],"role":"user"}],"step":2,"system":"sys","tools":[],"turn":"u1"},"dropped":3,"incomplete":true,"project":"p","reply":{"message":{"blocks":[{"kind":"text","text":"ok"}],"role":"assistant"},"message_id":"m1","stop_reason":"end_turn","usage":{"cache_read":null,"cache_write":null,"input":5,"output":1,"reasoning":null}},"session":"s","step":2,"thread":"t","turn":"u1"}"#
        );
    }

    #[test]
    fn each_optional_id_is_left_out_alone() {
        // It leaves out the ids that are none while keeping the ones that are set, in key order
        let record = Record {
            reply: reply(),
            session: Some("s".into()),
            turn: Some("u1".into()),
            ..Record::empty()
        };

        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"conversation":null,"dropped":0,"incomplete":false,"reply":{"message":{"blocks":[{"kind":"text","text":"ok"}],"role":"assistant"},"stop_reason":null,"usage":{"cache_read":null,"cache_write":null,"input":null,"output":null,"reasoning":null}},"session":"s","turn":"u1"}"#
        );
    }
}
