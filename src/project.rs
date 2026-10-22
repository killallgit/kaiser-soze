use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const CACHE_CAPACITY: usize = 256;

/// The git repository a session worked in, and the branch checked out when it was found.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub id: String,
    pub branch: Option<String>,
}

/// Finds the repository holding `dir`. A linked worktree is the repository it belongs to, on its own branch.
/// Only the repository's `HEAD` and `config` are read.
pub fn resolve(dir: &Path) -> Option<Project> {
    if !dir.is_dir() {
        return None;
    }
    let (work_tree, git_dir) = dir.ancestors().find_map(|candidate| {
        let git = candidate.join(".git");
        git_dir_of(&git).map(|git_dir| (candidate, git_dir))
    })?;
    let (config_dir, root) = match linked_main_git_dir(&git_dir) {
        Some(main_git_dir) => {
            let root = main_git_dir.parent()?.to_path_buf();
            (main_git_dir, root)
        }
        None => (git_dir.clone(), work_tree.to_path_buf()),
    };
    let id = fs::read_to_string(config_dir.join("config"))
        .ok()
        .and_then(|config| origin_url(&config))
        .map_or_else(
            || fs::canonicalize(&root).unwrap_or(root).display().to_string(),
            |url| without_credentials(&url),
        );
    Some(Project {
        id,
        branch: branch_of(&git_dir),
    })
}

fn git_dir_of(git: &Path) -> Option<PathBuf> {
    if git.is_dir() {
        return Some(git.to_path_buf());
    }
    let pointer = fs::read_to_string(git).ok()?;
    let target = Path::new(pointer.strip_prefix("gitdir:")?.trim());
    Some(git.parent()?.join(target))
}

/// The main repository's `.git` folder, when `git_dir` belongs to a linked worktree.
fn linked_main_git_dir(git_dir: &Path) -> Option<PathBuf> {
    let relative = fs::read_to_string(git_dir.join("commondir")).ok()?;
    fs::canonicalize(git_dir.join(relative.trim())).ok()
}

fn branch_of(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(str::to_string)
}

fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == "[remote \"origin\"]";
        } else if let Some((key, value)) = line.split_once('=').filter(|_| in_origin) {
            if key.trim() == "url" {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// `https://user:token@host/owner/repo.git?x#y` and `git@host:owner/repo.git` both become `host/owner/repo`.
fn without_credentials(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let without_extras = without_scheme.split(['?', '#']).next().unwrap_or_default();
    let host_and_path = without_extras.rsplit('@').next().unwrap_or_default();
    let scp_like = host_and_path
        .find(':')
        .is_some_and(|colon| !host_and_path[..colon].contains('/'));
    let id = if scp_like && !url.contains("://") && !url.starts_with(['/', '.']) {
        host_and_path.replacen(':', "/", 1)
    } else {
        host_and_path.to_string()
    };
    id.trim_end_matches('/').trim_end_matches(".git").to_string()
}

/// Remembers what each directory resolved to, so a repository is read once rather than once per request.
#[derive(Default)]
pub struct ProjectCache {
    found: Mutex<HashMap<PathBuf, Option<Project>>>,
}

impl ProjectCache {
    pub fn project_for(&self, dir: &Path) -> Option<Project> {
        if let Some(known) = self.found.lock().ok()?.get(dir) {
            return known.clone();
        }
        let project = resolve(dir);
        if let Ok(mut found) = self.found.lock() {
            if found.len() >= CACHE_CAPACITY {
                found.clear();
            }
            found.insert(dir.to_path_buf(), project.clone());
        }
        project
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn repository(root: &Path, head: &str, remote: Option<&str>) {
        write(&root.join(".git/HEAD"), head);
        let config = remote.map_or_else(String::new, |url| {
            format!("[core]\n\tbare = false\n[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n")
        });
        write(&root.join(".git/config"), &config);
    }

    fn on_main(root: &Path, remote: Option<&str>) {
        repository(root, "ref: refs/heads/main\n", remote);
    }

    fn project(id: &str, branch: Option<&str>) -> Option<Project> {
        Some(Project {
            id: id.into(),
            branch: branch.map(Into::into),
        })
    }

    #[test]
    fn a_repository_is_its_origin_remote_on_its_branch() {
        // It gives host/owner/repo and the checked-out branch
        let dir = tempfile::tempdir().unwrap();
        on_main(dir.path(), Some("https://example.com/owner/repo.git"));

        assert_eq!(resolve(dir.path()), project("example.com/owner/repo", Some("main")));
    }

    #[test]
    fn a_subdirectory_resolves_to_the_repository_above_it() {
        // It walks up to the nearest .git
        let dir = tempfile::tempdir().unwrap();
        on_main(dir.path(), Some("https://example.com/owner/repo.git"));
        let nested = dir.path().join("src/deep");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(resolve(&nested), project("example.com/owner/repo", Some("main")));
    }

    #[test]
    fn credentials_in_a_remote_are_removed() {
        // It keeps neither the user, the password nor the scheme
        let remotes = [
            "https://user:token@example.com/owner/repo.git",
            "ssh://git@example.com/owner/repo",
            "git@example.com:owner/repo.git",
            "https://example.com/owner/repo/",
            "https://u:pa/ss@example.com/owner/repo",
            "user:pass@example.com/owner/repo",
            "https://example.com/owner/repo.git?token=abc",
            "https://example.com/owner/repo#frag",
        ];
        for remote in remotes {
            let dir = tempfile::tempdir().unwrap();
            on_main(dir.path(), Some(remote));

            let found = resolve(dir.path());

            assert_eq!(found, project("example.com/owner/repo", Some("main")), "{remote}");
            for secret in ["pass", "pa/ss", "token", "abc", "frag"] {
                assert!(!found.as_ref().unwrap().id.contains(secret), "{remote}");
            }
        }
    }

    #[test]
    fn no_remote_gives_the_resolved_path() {
        // It names the repository by where it is
        let dir = tempfile::tempdir().unwrap();
        on_main(dir.path(), None);

        assert_eq!(
            resolve(dir.path()),
            project(
                &fs::canonicalize(dir.path()).unwrap().display().to_string(),
                Some("main")
            )
        );
    }

    #[test]
    fn a_linked_worktree_is_the_main_repository_on_its_own_branch() {
        // It shares the project id and keeps the worktree's branch
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let linked = dir.path().join("linked");
        on_main(&main, Some("https://example.com/owner/repo.git"));
        let admin = main.join(".git/worktrees/linked");
        write(&admin.join("HEAD"), "ref: refs/heads/feature\n");
        write(&admin.join("commondir"), "../..\n");
        write(&linked.join(".git"), &format!("gitdir: {}\n", admin.display()));

        assert_eq!(resolve(&linked), project("example.com/owner/repo", Some("feature")));
    }

    #[test]
    fn a_linked_worktree_without_a_remote_is_named_by_the_main_repository_path() {
        // It does not name the project after the worktree's own folder
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let linked = dir.path().join("linked");
        on_main(&main, None);
        let admin = main.join(".git/worktrees/linked");
        write(&admin.join("HEAD"), "ref: refs/heads/feature\n");
        write(&admin.join("commondir"), "../..\n");
        write(&linked.join(".git"), &format!("gitdir: {}\n", admin.display()));

        let found = resolve(&linked).unwrap();

        assert_eq!(Path::new(&found.id), fs::canonicalize(&main).unwrap());
    }

    #[test]
    fn a_detached_head_gives_no_branch() {
        // It keeps the project and leaves the branch out
        let dir = tempfile::tempdir().unwrap();
        repository(
            dir.path(),
            "0123456789abcdef0123456789abcdef01234567\n",
            Some("https://example.com/owner/repo"),
        );

        assert_eq!(resolve(dir.path()), project("example.com/owner/repo", None));
    }

    #[test]
    fn a_directory_in_no_repository_gives_none() {
        // It finds no project
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(resolve(dir.path()), None);
    }

    #[test]
    fn a_missing_directory_gives_none() {
        // It finds no project, even under a repository
        let dir = tempfile::tempdir().unwrap();
        on_main(dir.path(), Some("https://example.com/owner/repo"));

        assert_eq!(resolve(&dir.path().join("gone")), None);
    }

    #[test]
    fn the_cache_answers_again_without_reading_the_disk() {
        // It returns the stored answer after the repository is gone
        let dir = tempfile::tempdir().unwrap();
        on_main(dir.path(), Some("https://example.com/owner/repo"));
        let cache = ProjectCache::default();
        let first = cache.project_for(dir.path());

        fs::remove_dir_all(dir.path().join(".git")).unwrap();

        assert_eq!(cache.project_for(dir.path()), first);
        assert!(first.is_some());
    }

    #[test]
    fn the_cache_stays_within_its_capacity() {
        // It forgets old directories instead of growing
        let dir = tempfile::tempdir().unwrap();
        let cache = ProjectCache::default();

        for n in 0..CACHE_CAPACITY * 2 {
            cache.project_for(&dir.path().join(n.to_string()));
        }

        assert!(cache.found.lock().unwrap().len() <= CACHE_CAPACITY);
    }
}
