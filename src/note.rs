use crate::label::Label;
use serde::Serialize;

/// What the proxy says about an exchange, or about a connection that had none. The structs declare
/// their fields in the key order the tap file writes, and a note serialises as its payload alone:
/// the event name is the tap line's own key.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Note {
    ExchangeStart(ExchangeStart),
    ResponseHead(ResponseHead),
    ExchangeEnd(ExchangeEnd),
    Tunnel(Tunnel),
    Unsupported(Unsupported),
}

impl Note {
    /// The name the tap line carries under its `event` key.
    pub fn event(&self) -> &'static str {
        match self {
            Self::ExchangeStart(_) => "exchange_start",
            Self::ResponseHead(_) => "response_head",
            Self::ExchangeEnd(_) => "exchange_end",
            Self::Tunnel(_) => "tunnel",
            Self::Unsupported(_) => "unsupported",
        }
    }
}

/// A request an adapter claimed, with its redacted headers. A missing label is left out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExchangeStart {
    pub headers: Vec<(String, String)>,
    pub host: String,
    pub http_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    pub method: String,
    pub path: String,
    pub upgrade: bool,
}

/// How the reply is carried, as far as its status and content type say.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Http,
    Sse,
    Websocket,
}

/// The status line and redacted headers of a reply.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResponseHead {
    pub headers: Vec<(String, String)>,
    pub http_version: String,
    pub status: u16,
    pub transport: Transport,
}

/// What an exchange cost the tap. A moment that never came is `null`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExchangeEnd {
    pub bytes: u64,
    pub chunks: u64,
    pub first_byte_us: Option<u64>,
    pub forward_max_us: u64,
    pub forward_sum_us: u64,
    pub response_head_us: Option<u64>,
    pub sink_dropped: u64,
    pub tap_dropped: u64,
}

/// A connection to a host no adapter claims, passed through untouched. A missing label is left out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tunnel {
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    pub port: u16,
}

/// A request to a claimed host that the adapter does not read. A missing label is left out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unsupported {
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    pub path: String,
}

#[cfg(test)]
mod tests;
