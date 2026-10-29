pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub mod adapter;
pub mod assembler;
mod capture;
pub mod common;
pub mod exchange;
pub mod graph;
pub mod logging;
pub mod project;
pub mod proxy;
pub mod shape;
pub mod sink;

pub use adapter::{claude, codex};
pub use common::{event, line, message, note, record};
