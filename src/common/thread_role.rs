use serde::Serialize;

/// Whether a thread is the main agent or a subagent; `Unknown` unless the headers say so outright.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadRole {
    Main,
    Subagent,
    #[default]
    Unknown,
}
