use crate::adapter::{header, Adapter, ExchangeParser, RequestHead};
use crate::graph::Identity;

mod parser;
mod request;
mod stream;
#[cfg(test)]
mod tests;

pub use parser::ClaudeParser;

pub struct ClaudeAdapter {
    hosts: Vec<String>,
}

impl ClaudeAdapter {
    pub fn new(hosts: Vec<String>) -> Self {
        Self { hosts }
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new(vec!["api.anthropic.com".into()])
    }
}

impl Adapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn hosts(&self) -> &[String] {
        &self.hosts
    }

    fn claim(&self, head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        let path = head.path.split('?').next().unwrap_or_default();
        (path == MESSAGES_PATH).then(|| Box::new(ClaudeParser::new(head)) as Box<dyn ExchangeParser>)
    }

    /// The main agent has no agent id and is the session itself; a subagent's parent is the agent that
    /// launched it, else the main agent.
    fn identity(&self, head: &RequestHead) -> Identity {
        let owned = |name| header(head, name).map(str::to_string);
        let session = owned("x-claude-code-session-id");
        let thread = owned("x-claude-code-agent-id").or_else(|| session.clone());
        let subagent = header(head, "x-claude-code-request-class") == Some("subagent");
        let parent_thread = owned("x-claude-code-parent-agent-id").or(session.clone().filter(|_| subagent));
        Identity {
            session,
            thread,
            parent_thread,
            turn: owned("x-claude-code-prompt-id"),
        }
    }
}

const MESSAGES_PATH: &str = "/v1/messages";
