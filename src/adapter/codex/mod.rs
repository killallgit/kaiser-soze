use crate::adapter::body::is_encoded;
use crate::adapter::{header, Adapter, ExchangeParser, RequestHead};
use crate::graph::{named_turn, Identity, ThreadRole};
use serde_json::Value;

mod history;
mod parser;
mod request;
mod stream;
#[cfg(test)]
mod tests;

pub use parser::CodexParser;

pub struct CodexAdapter {
    hosts: Vec<String>,
}

impl CodexAdapter {
    pub fn new(hosts: Vec<String>) -> Self {
        Self { hosts }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new(vec!["chatgpt.com".into(), "api.openai.com".into()])
    }
}

impl Adapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn hosts(&self) -> &[String] {
        &self.hosts
    }

    fn claim(&self, head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        head.path
            .ends_with("/responses")
            .then(|| Box::new(CodexParser::new(is_encoded(&head.headers))) as Box<dyn ExchangeParser>)
    }

    fn identity(&self, head: &RequestHead) -> Identity {
        let owned = |name| header(head, name).map(str::to_string);
        let turn_metadata =
            header(head, "x-codex-turn-metadata").and_then(|json| serde_json::from_str::<Value>(json).ok());
        let (session, thread) = (owned("session-id"), owned("thread-id"));
        let parent_thread = owned("x-codex-parent-thread-id");
        let spawned = parent_thread.is_some() || header(head, "x-openai-subagent").is_some_and(|name| !name.is_empty());
        let role = match (spawned, &session, &thread) {
            (true, _, _) => ThreadRole::Subagent,
            (false, Some(session), Some(thread)) if session == thread => ThreadRole::Main,
            _ => ThreadRole::Unknown,
        };
        Identity {
            session,
            thread,
            parent_thread,
            turn: named_turn(turn_metadata.as_ref().and_then(|metadata| metadata["turn_id"].as_str())),
            role,
        }
    }
}
