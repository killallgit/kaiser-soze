use super::message::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request: the messages it sent, with the system prompt and tools as sent. The optional fields
/// are left out when missing, and the two flags when false.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub partial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    pub system: Value,
    pub tools: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub warmup: bool,
}

fn is_false(flag: &bool) -> bool {
    !flag
}

impl Conversation {
    pub fn new(system: Value, tools: Value, messages: Vec<Message>) -> Self {
        Self {
            messages,
            partial: false,
            previous_message_id: None,
            step: None,
            system,
            tools,
            turn: None,
            warmup: false,
        }
    }
}
