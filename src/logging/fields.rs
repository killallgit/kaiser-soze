use serde::Serialize;
use std::io;

/// Explicit metadata projection. There is no free-form message, body, headers or tool-input field.
#[derive(Clone, Default, Serialize)]
pub struct Fields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tap_dropped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sink_dropped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_head_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_max_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<u64>,
}

impl Fields {
    pub fn io(error: &io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::NotFound => "not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            io::ErrorKind::StorageFull => "storage_full",
            io::ErrorKind::BrokenPipe => "broken_pipe",
            io::ErrorKind::WriteZero => "write_zero",
            io::ErrorKind::ConnectionRefused => "connection_refused",
            io::ErrorKind::TimedOut => "timed_out",
            _ => "io",
        };
        Self {
            error_kind: Some(kind),
            ..Self::default()
        }
    }

    pub(super) fn bound(&mut self) {
        if let Some(path) = self.ca_path.as_mut() {
            *path = path.chars().filter(|c| !c.is_control()).take(160).collect();
        }
    }
}
