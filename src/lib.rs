pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub mod adapter;
pub mod body;
pub mod ca;
pub mod claude;
pub mod codex;
pub mod exchange;
pub mod proxy;
pub mod redact;
pub mod sink;
pub mod sse;
pub mod ws;
