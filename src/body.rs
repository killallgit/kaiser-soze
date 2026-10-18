use crate::adapter::TapEvent;
use serde_json::{json, Value};

pub const MAX_REQUEST_BODY: usize = 32 * 1024 * 1024;

pub fn event(kind: &str, payload: Value, start: u64, end: u64) -> TapEvent {
    TapEvent {
        kind: kind.to_string(),
        payload,
        start,
        end,
    }
}

pub fn is_encoded(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name == "content-encoding" && value != "identity")
}

pub fn tool_names(tools: &Value) -> Vec<Value> {
    tools
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool.get("name").or(tool.get("type")).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .unwrap_or_default()
}

pub struct RequestBuffer {
    bytes: Vec<u8>,
    truncated: bool,
    encoded: bool,
}

impl RequestBuffer {
    pub fn new(encoded: bool) -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
            encoded,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        if self.bytes.len() + chunk.len() <= MAX_REQUEST_BODY {
            self.bytes.extend_from_slice(chunk);
        } else {
            self.truncated = true;
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn unreadable(&self) -> Option<TapEvent> {
        (self.truncated || self.encoded).then(|| {
            let payload = json!({"bytes": self.bytes.len(), "truncated": self.truncated, "encoded": self.encoded});
            event("request_unparsed", payload, 0, self.bytes.len() as u64)
        })
    }
}
