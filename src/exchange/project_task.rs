use super::clock::Clock;
use super::recorder::Recorder;
use crate::graph::{self, Identity};
use crate::project::ProjectLookups;
use std::path::PathBuf;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// `None` until the project task settles; then `Some` of the project it found.
type Settling = Option<Option<String>>;

/// The project an exchange's records carry, as its project task settles it.
pub(super) struct Project(watch::Receiver<Settling>);

impl Project {
    /// Waits for the project task to settle; a task that ended without settling leaves no project.
    pub(super) async fn settled(&self) -> Option<String> {
        let mut settling = self.0.clone();
        let settled = settling.wait_for(Option::is_some).await.ok()?.clone();
        settled.flatten()
    }
}

/// Finds the project of the exchange's directory, settles it for the records, then writes the start edges.
pub(super) struct ProjectTask {
    pub recorder: Recorder,
    pub clock: Clock,
    pub identity: Identity,
    pub dir: Option<PathBuf>,
    pub projects: ProjectLookups,
}

impl ProjectTask {
    /// Spawns the task; its handle resolves once the start edges are written.
    pub(super) fn spawn(self) -> (Project, JoinHandle<()>) {
        let (settle, settling) = watch::channel(None);
        (Project(settling), tokio::spawn(self.run(settle)))
    }

    async fn run(self, settle: watch::Sender<Settling>) {
        let project = match self.dir {
            Some(dir) => self.projects.find(dir).await,
            None => None,
        };
        settle.send_replace(Some(project.as_ref().map(|project| project.id.clone())));
        for edge in graph::edges(self.recorder.exchange, &self.identity, project.as_ref()) {
            self.recorder.write_edge(self.clock.stamp(), edge).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::TapOutput;
    use crate::project::ProjectCache;
    use std::time::Duration;

    #[tokio::test]
    async fn a_project_task_cancelled_before_it_settles_leaves_no_project() {
        // Records waiting on a task that can no longer answer go ahead with no project
        let (output, _drained) = TapOutput::detached(1).unwrap();
        let (project, start_edges) = ProjectTask {
            recorder: Recorder::new(output, 1, "test"),
            clock: Clock::start(),
            identity: Identity::default(),
            dir: Some("/nowhere".into()),
            projects: ProjectLookups::new(ProjectCache::default()),
        }
        .spawn();

        start_edges.abort();
        let settled = tokio::time::timeout(Duration::from_secs(10), project.settled()).await;

        assert_eq!(settled, Ok(None));
    }
}
