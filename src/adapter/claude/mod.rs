use crate::adapter::{header, Adapter, ExchangeParser, RequestHead};
use crate::graph::{Identity, ThreadRole};

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
    /// launched it, else the main agent. A subagent request with no agent id has no parent, since its
    /// thread would be the session itself.
    fn identity(&self, head: &RequestHead) -> Identity {
        let owned = |name| header(head, name).map(str::to_string);
        let session = owned("x-claude-code-session-id");
        let agent = owned("x-claude-code-agent-id");
        let thread = agent.clone().or_else(|| session.clone());
        let class = header(head, "x-claude-code-request-class");
        let subagent = class == Some("subagent");
        let role = match (class, agent.is_some()) {
            (Some("main"), false) => ThreadRole::Main,
            (Some("subagent"), true) => ThreadRole::Subagent,
            _ => ThreadRole::Unknown,
        };
        let parent_thread =
            owned("x-claude-code-parent-agent-id").or(session.clone().filter(|_| subagent && agent.is_some()));
        Identity {
            session,
            thread,
            parent_thread,
            turn: owned("x-claude-code-prompt-id"),
            role,
        }
    }
}

const MESSAGES_PATH: &str = "/v1/messages";
