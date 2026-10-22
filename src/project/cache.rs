use super::repository::{locate, Repository};
use super::Project;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(super) const CACHE_CAPACITY: usize = 256;
const NEGATIVE_TTL: Duration = Duration::from_secs(60);

type Locate = Box<dyn Fn(&Path) -> Option<Repository> + Send + Sync>;

pub(super) struct Entry {
    pub(super) repository: Option<Repository>,
    pub(super) found_at: Instant,
}

/// Remembers which repository each directory belongs to, so its `config` is read once rather than once per request.
/// The branch is not remembered: a checkout changes it, so every lookup reads `HEAD` again.
/// A directory in no repository is looked up again after `negative_ttl`, since `git init` may follow.
pub struct ProjectCache {
    pub(super) found: Mutex<HashMap<PathBuf, Entry>>,
    locate: Locate,
    negative_ttl: Duration,
}

impl Default for ProjectCache {
    fn default() -> Self {
        Self::locating_with(locate)
    }
}

impl ProjectCache {
    /// A cache that finds repositories with `locate` instead of reading the disk.
    pub fn locating_with(locate: impl Fn(&Path) -> Option<Repository> + Send + Sync + 'static) -> Self {
        Self {
            found: Mutex::default(),
            locate: Box::new(locate),
            negative_ttl: NEGATIVE_TTL,
        }
    }

    pub fn with_negative_ttl(mut self, negative_ttl: Duration) -> Self {
        self.negative_ttl = negative_ttl;
        self
    }

    pub fn project_for(&self, dir: &Path) -> Option<Project> {
        let known = self
            .found
            .lock()
            .ok()?
            .get(dir)
            .filter(|entry| !self.expired(entry))
            .map(|entry| entry.repository.clone());
        if let Some(known) = known {
            return known.as_ref().map(Repository::project);
        }
        let repository = (self.locate)(dir);
        if let Ok(mut found) = self.found.lock() {
            if found.len() >= CACHE_CAPACITY && !found.contains_key(dir) {
                found.clear();
            }
            found.insert(
                dir.to_path_buf(),
                Entry {
                    repository: repository.clone(),
                    found_at: Instant::now(),
                },
            );
        }
        repository.as_ref().map(Repository::project)
    }

    fn expired(&self, entry: &Entry) -> bool {
        entry.repository.is_none() && entry.found_at.elapsed() >= self.negative_ttl
    }
}
