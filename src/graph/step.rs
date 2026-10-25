use super::Identity;
use serde_json::Value;

/// One request and its reply: the unit `turn` and `continues` link.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub thread: Option<String>,
    pub exchange: u64,
    pub n: u64,
    pub turn: Option<String>,
    pub previous_message_id: Option<String>,
    pub message_id: Option<String>,
    /// Call ids of the `tool_result` blocks in the request.
    pub results: Vec<String>,
    /// Ids of the `tool_call` blocks in the reply.
    pub calls: Vec<String>,
}

impl Step {
    /// The step a finished record describes, or none when the record names no step. A first step
    /// that names no turn is in the header turn; a later one may be in a new turn, so it has none.
    pub fn of_record(record: &Value, identity: &Identity, exchange: u64) -> Option<Step> {
        let n = record["step"].as_u64()?;
        let text = |value: &Value| value.as_str().map(String::from);
        let header_turn = (record.get("turn").is_none() && n == 1)
            .then(|| identity.turn.clone())
            .flatten();
        Some(Step {
            thread: identity.thread.clone(),
            exchange,
            n,
            turn: text(&record["turn"]).or(header_turn),
            previous_message_id: text(&record["conversation"]["previous_message_id"]),
            message_id: text(&record["reply"]["message_id"]),
            results: block_ids(&record["conversation"]["messages"], "tool_result", "call_id"),
            calls: block_ids(&record["reply"]["message"], "tool_call", "id"),
        })
    }
}

/// The turn a request names; an empty id, as a warm-up request sends, names none.
pub fn named_turn(turn: Option<&str>) -> Option<String> {
    turn.filter(|turn| !turn.is_empty()).map(String::from)
}

/// The `key` of each block of `kind` in a message or a list of messages.
fn block_ids(messages: &Value, kind: &str, key: &str) -> Vec<String> {
    let messages = match messages {
        Value::Array(messages) => messages.as_slice(),
        message => std::slice::from_ref(message),
    };
    messages
        .iter()
        .filter_map(|message| message["blocks"].as_array())
        .flatten()
        .filter(|block| block["kind"] == kind)
        .filter_map(|block| block[key].as_str().map(String::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity(turn: Option<&str>) -> Identity {
        Identity {
            thread: Some("t1".into()),
            turn: turn.map(Into::into),
            ..Identity::default()
        }
    }

    #[test]
    fn an_empty_turn_id_is_no_turn() {
        // The prewarm request sends turn_id as an empty string
        assert_eq!(named_turn(Some("")), None);
        assert_eq!(named_turn(None), None);
        assert_eq!(named_turn(Some("u1")).as_deref(), Some("u1"));
    }

    #[test]
    fn a_first_step_that_names_no_turn_takes_the_header_turn() {
        // The header turn is the first request's
        let record = json!({"step": 1});

        let step = Step::of_record(&record, &identity(Some("h1")), 7).unwrap();

        assert_eq!(step.turn.as_deref(), Some("h1"));
        assert_eq!((step.thread.as_deref(), step.exchange, step.n), (Some("t1"), 7, 1));
    }

    #[test]
    fn an_explicit_turn_beats_the_header_turn() {
        // A Codex step names its own turn, whichever step it is
        let first = Step::of_record(&json!({"step": 1, "turn": "u2"}), &identity(Some("h1")), 7).unwrap();
        let later = Step::of_record(&json!({"step": 3, "turn": "u2"}), &identity(Some("h1")), 7).unwrap();

        assert_eq!(first.turn.as_deref(), Some("u2"));
        assert_eq!(later.turn.as_deref(), Some("u2"));
    }

    #[test]
    fn a_later_step_that_names_no_turn_gets_none() {
        // The header turn does not carry past the first step
        let step = Step::of_record(&json!({"step": 2}), &identity(Some("h1")), 7).unwrap();

        assert_eq!(step.turn, None);
    }

    #[test]
    fn a_record_that_names_no_step_gives_no_step() {
        // A reply with no request in front of it cannot be linked
        assert_eq!(Step::of_record(&json!({"reply": {}}), &identity(Some("h1")), 7), None);
    }

    #[test]
    fn the_message_ids_and_the_call_ids_are_read_from_the_record() {
        // It reads the previous and reply message ids, the result ids of the request and the call ids of the reply
        let record = json!({
            "step": 2,
            "conversation": {
                "previous_message_id": "m1",
                "messages": [
                    {"role": "user", "blocks": [{"kind": "tool_result", "call_id": "c1"}, {"kind": "text", "text": "x"}]},
                    {"role": "user", "blocks": [{"kind": "tool_result", "call_id": "c2"}]},
                ],
            },
            "reply": {
                "message_id": "m2",
                "message": {"role": "assistant", "blocks": [{"kind": "tool_call", "id": "c3"}, {"kind": "text", "text": "y"}]},
            },
        });

        let step = Step::of_record(&record, &identity(None), 7).unwrap();

        assert_eq!(step.previous_message_id.as_deref(), Some("m1"));
        assert_eq!(step.message_id.as_deref(), Some("m2"));
        assert_eq!(step.results, ["c1", "c2"]);
        assert_eq!(step.calls, ["c3"]);
    }

    #[test]
    fn a_record_with_none_of_the_ids_gives_a_bare_step() {
        // Missing ids are absent, not empty
        let step = Step::of_record(&json!({"step": 1}), &Identity::default(), 7).unwrap();

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
