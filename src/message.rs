use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use serde_json::Value;

/// Who a message is from. The upstream role passes through as sent, so any string is a role.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Role(String);

impl Role {
    pub fn user() -> Self {
        Self::from("user")
    }

    pub fn assistant() -> Self {
        Self::from("assistant")
    }
}

impl From<&str> for Role {
    fn from(role: &str) -> Self {
        Self(role.to_string())
    }
}

/// One turn of a conversation. The fields are declared in the order the tap file writes them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    pub blocks: Vec<Block>,
    pub role: Role,
}

/// The output of a tool result: the blocks it holds, or the upstream value as sent.
#[derive(Clone, Debug, PartialEq, Serialize)]
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

#[cfg(test)]
mod tests;
