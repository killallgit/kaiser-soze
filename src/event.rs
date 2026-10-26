use crate::message::Message;
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Text,
    Reasoning,
    ToolCall,
    Other,
}

/// A reply block opening. A `null` id or name is left out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockStart {
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub id: Value,
    pub index: u64,
    pub kind: BlockKind,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub name: Value,
}

/// One piece of an open block: its index and the field named for the kind of piece. The two keys
/// are written sorted, so the index follows `encrypted` and comes before the other fields.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct BlockDelta {
    pub index: u64,
    #[serde(flatten)]
    pub delta: Delta,
}

impl Serialize for BlockDelta {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (field, piece) = self.delta.field();
        let mut delta = serializer.serialize_map(Some(2))?;
        if field < "index" {
            delta.serialize_entry(field, piece)?;
            delta.serialize_entry("index", &self.index)?;
        } else {
            delta.serialize_entry("index", &self.index)?;
            delta.serialize_entry(field, piece)?;
        }
        delta.end()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    Text(String),
    InputJson(String),
    Signature(String),
    Encrypted(String),
}

impl Delta {
    /// The key the delta is written under, and the piece it carries.
    fn field(&self) -> (&'static str, &str) {
        match self {
            Self::Text(piece) => ("text", piece),
            Self::InputJson(piece) => ("input_json", piece),
            Self::Signature(piece) => ("signature", piece),
            Self::Encrypted(piece) => ("encrypted", piece),
        }
    }

    /// The text the delta carries, whichever kind it is.
    pub fn piece(&self) -> &str {
        match self {
            Self::Text(piece) | Self::InputJson(piece) | Self::Signature(piece) | Self::Encrypted(piece) => piece,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockStop {
    pub index: u64,
}

/// The token counts one event reports; a count the upstream did not send is `null`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplyEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Value>,
    pub stop_reason: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub error_type: Value,
}

/// A request or a reply the adapter could not read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unparsed {
    pub side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Request,
    Reply,
}

impl Side {
    /// The name the side is written under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Reply => "reply",
        }
    }
}

#[cfg(test)]
impl Event {
    /// The conversation an event holds; any other event fails the test.
    pub(crate) fn conversation(&self) -> &Conversation {
        match self {
            Self::Conversation(conversation) => conversation,
            other => panic!("expected a conversation, got {}", other.kind()),
        }
    }

    pub(crate) fn conversation_mut(&mut self) -> &mut Conversation {
        match self {
            Self::Conversation(conversation) => conversation,
            other => panic!("expected a conversation, got {}", other.kind()),
        }
    }
}

#[cfg(test)]
mod tests;
