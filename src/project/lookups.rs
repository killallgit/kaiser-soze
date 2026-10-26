use super::{Project, ProjectCache};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const MAX_LOOKUPS: usize = 4;
const LOOKUP_WAIT: Duration = Duration::from_millis(250);

/// Finds the project of a directory on a blocking thread and never waits long for it.
/// A lookup that outlasts the wait finds nothing and keeps running, holding one of `MAX_LOOKUPS` permits,
/// so a hung filesystem cannot fill the blocking pool; at the cap a lookup finds nothing at once.
#[derive(Clone)]
pub struct ProjectLookups {
    cache: Arc<ProjectCache>,
    permits: Arc<Semaphore>,
    wait: Duration,
}

impl ProjectLookups {
    pub fn new(cache: ProjectCache) -> Self {
        Self {
            cache: Arc::new(cache),
            permits: Arc::new(Semaphore::new(MAX_LOOKUPS)),
            wait: LOOKUP_WAIT,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    pub async fn find(&self, dir: PathBuf) -> Option<Project> {
        let permit = self.permits.clone().try_acquire_owned().ok()?;
        let cache = self.cache.clone();
        let lookup = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            cache.project_for(&dir)
        });
        tokio::time::timeout(self.wait, lookup).await.ok()?.ok()?
    }
}
