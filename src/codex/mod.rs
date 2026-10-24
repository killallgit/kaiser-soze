use crate::adapter::{header, Adapter, ExchangeParser, RequestHead};
use crate::body::is_encoded;
use crate::graph::Identity;
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
        Identity {
            session: owned("session-id"),
            thread: owned("thread-id"),
            parent_thread: owned("x-codex-parent-thread-id"),
            turn: turn_metadata
                .and_then(|metadata| metadata["turn_id"].as_str().map(str::to_string))
                .filter(|turn| !turn.is_empty()),
        }
    }
}
