mod cache;
mod id;
mod repository;
#[cfg(test)]
mod tests;

pub use cache::ProjectCache;
pub use repository::{locate, Repository};

/// The git repository a session worked in, and the branch checked out when it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub id: String,
    pub branch: Option<String>,
}
