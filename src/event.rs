use crate::message::Message;
use serde::Serialize;
use serde_json::Value;

/// What an adapter says about one exchange. The payload structs declare their fields in the key
/// order the tap file writes, and an event serialises as its payload alone: the event name is the
/// tap line's own key.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Event {
    Conversation(Conversation),
    BlockStart(BlockStart),
    BlockDelta(BlockDelta),
    BlockStop(BlockStop),
    Usage(Usage),
    ReplyEnd(ReplyEnd),
    Error(Error),
    Unparsed(Unparsed),
}

impl Event {
    /// The name the tap line carries under its `event` key.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Conversation(_) => "conversation",
            Self::BlockStart(_) => "block_start",
            Self::BlockDelta(_) => "block_delta",
            Self::BlockStop(_) => "block_stop",
            Self::Usage(_) => "usage",
            Self::ReplyEnd(_) => "reply_end",
            Self::Error(_) => "error",
            Self::Unparsed(_) => "unparsed",
        }
    }
}

/// A request: the messages it sent, with the system prompt and tools as sent. The optional fields
/// are left out when missing, and the two flags when false.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    pub system: Value,
    pub tools: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Text,
    Reasoning,
    ToolCall,
    Other,
}

/// A reply block opening. A `null` id or name is left out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockStart {
    #[serde(skip_serializing_if = "Value::is_null")]
    pub id: Value,
    pub index: u64,
    pub kind: BlockKind,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub name: Value,
}

/// One piece of an open block: its index, then the field named for the kind of piece.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockDelta {
    pub index: u64,
    #[serde(flatten)]
    pub delta: Delta,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    Text(String),
    InputJson(String),
    Signature(String),
    Encrypted(String),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockStop {
    pub index: u64,
}

/// The token counts one event reports; a count the upstream did not send is `null`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub reasoning: Option<u64>,
}

impl Usage {
    /// Takes every count the later report has and keeps the rest.
    pub fn merge(&mut self, later: Usage) {
        self.cache_read = later.cache_read.or(self.cache_read);
        self.cache_write = later.cache_write.or(self.cache_write);
        self.input = later.input.or(self.input);
        self.output = later.output.or(self.output);
        self.reasoning = later.reasoning.or(self.reasoning);
    }
}

/// The end of a reply. The stop reason passes through as sent; a missing message id is left out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReplyEnd {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Value>,
    pub stop_reason: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Error {
    pub error_type: Value,
}

/// A request or a reply the adapter could not read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unparsed {
    pub side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Request,
    Reply,
}

#[cfg(test)]
mod tests;
