use serde::de::Error;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// Who a message is from. The upstream role passes through as sent, so any string is a role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Role(String);

impl Role {
    fn user() -> Self {
        Self::from("user")
    }

    fn assistant() -> Self {
        Self::from("assistant")
    }
}

impl From<&str> for Role {
    fn from(role: &str) -> Self {
        Self(role.to_string())
    }
}

/// One turn of a conversation. The fields are declared in the order the tap file writes them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub blocks: Vec<Block>,
    pub role: Role,
}

impl Message {
    pub fn user(blocks: Vec<Block>) -> Self {
        Self {
            blocks,
            role: Role::user(),
        }
    }

    pub fn assistant(blocks: Vec<Block>) -> Self {
        Self {
            blocks,
            role: Role::assistant(),
        }
    }
}

/// The output of a tool result: the blocks it holds, or the upstream value as sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Output {
    Blocks(Vec<Block>),
    Raw(Value),
}

/// One piece of a message. Ids, names, inputs and outputs keep the upstream JSON as sent, so a
/// missing one stays `null`; only a reasoning signature is left out when it is missing.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text {
        text: String,
    },
    Reasoning {
        text: Value,
        signature: Option<Value>,
    },
    EncryptedReasoning {
        encrypted: Value,
    },
    ToolCall {
        id: Value,
        input: Value,
        name: Value,
    },
    ToolResult {
        call_id: Value,
        is_error: bool,
        output: Output,
    },
    Image {
        content_type: String,
        reference: String,
        size: usize,
    },
}

impl Block {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// A reasoning block; a `null` signature is no signature.
    pub fn reasoning(text: Value, signature: Value) -> Self {
        Self::Reasoning {
            text,
            signature: Some(signature).filter(|signature| !signature.is_null()),
        }
    }
}

/// Writes the keys in sorted order with `kind` among them, which an internally tagged derive cannot.
/// The conversation payload is sorted by its `Value` map anyway; this keeps a block serialised directly,
/// as a record that serialises blocks directly would, in the same order.
impl Serialize for Block {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Block::Text { text } => {
                let mut block = serializer.serialize_struct("Block", 2)?;
                block.serialize_field("kind", "text")?;
                block.serialize_field("text", text)?;
                block.end()
            }
            Block::Reasoning { text, signature } => {
                let mut block = serializer.serialize_struct("Block", 2 + usize::from(signature.is_some()))?;
                block.serialize_field("kind", "reasoning")?;
                if let Some(signature) = signature {
                    block.serialize_field("signature", signature)?;
                }
                block.serialize_field("text", text)?;
                block.end()
            }
            Block::EncryptedReasoning { encrypted } => {
                let mut block = serializer.serialize_struct("Block", 2)?;
                block.serialize_field("encrypted", encrypted)?;
                block.serialize_field("kind", "reasoning")?;
                block.end()
            }
            Block::ToolCall { id, input, name } => {
                let mut block = serializer.serialize_struct("Block", 4)?;
                block.serialize_field("id", id)?;
                block.serialize_field("input", input)?;
                block.serialize_field("kind", "tool_call")?;
                block.serialize_field("name", name)?;
                block.end()
            }
            Block::ToolResult {
                call_id,
                is_error,
                output,
            } => {
                let mut block = serializer.serialize_struct("Block", 4)?;
                block.serialize_field("call_id", call_id)?;
                block.serialize_field("is_error", is_error)?;
                block.serialize_field("kind", "tool_result")?;
                block.serialize_field("output", output)?;
                block.end()
            }
            Block::Image {
                content_type,
                reference,
                size,
            } => {
                let mut block = serializer.serialize_struct("Block", 4)?;
                block.serialize_field("content_type", content_type)?;
                block.serialize_field("kind", "image")?;
                block.serialize_field("ref", reference)?;
                block.serialize_field("size", size)?;
                block.end()
            }
        }
    }
}

/// Reads a block back from what `Serialize` wrote, so a tap line can be replayed. A reasoning block
/// with an `encrypted` key is the encrypted kind, since both write the kind `reasoning`.
impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = Map::<String, Value>::deserialize(deserializer)?;
        let field = |name: &str| fields.get(name).cloned().unwrap_or(Value::Null);
        let string = |name: &'static str| {
            field(name)
                .as_str()
                .map(String::from)
                .ok_or_else(|| D::Error::missing_field(name))
        };
        Ok(match field("kind").as_str() {
            Some("text") => Block::Text { text: string("text")? },
            Some("reasoning") if fields.contains_key("encrypted") => Block::EncryptedReasoning {
                encrypted: field("encrypted"),
            },
            Some("reasoning") => Block::reasoning(field("text"), field("signature")),
            Some("tool_call") => Block::ToolCall {
                id: field("id"),
                input: field("input"),
                name: field("name"),
            },
            Some("tool_result") => Block::ToolResult {
                call_id: field("call_id"),
                is_error: field("is_error") == Value::Bool(true),
                output: serde_json::from_value(field("output")).map_err(D::Error::custom)?,
            },
            Some("image") => Block::Image {
                content_type: string("content_type")?,
                reference: string("ref")?,
                size: field("size").as_u64().ok_or_else(|| D::Error::missing_field("size"))? as usize,
            },
            _ => return Err(D::Error::missing_field("kind")),
        })
    }
}

#[cfg(test)]
mod tests;
