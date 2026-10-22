use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_CAPACITY: usize = 256;
const NEGATIVE_TTL: Duration = Duration::from_secs(60);

/// The git repository a session worked in, and the branch checked out when it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub id: String,
    pub branch: Option<String>,
}

/// A repository found for a directory: its stable id, and where its own `HEAD` lives.
#[derive(Clone, Debug)]
pub struct Repository {
    id: String,
    git_dir: PathBuf,
}

impl Repository {
    fn project(&self) -> Project {
        Project {
            id: self.id.clone(),
            branch: branch_of(&self.git_dir),
        }
    }
}

/// Finds the repository holding `dir`. A linked worktree is the repository it belongs to, with its own `HEAD`.
/// Only the repository's `HEAD` and `config` are read.
pub fn locate(dir: &Path) -> Option<Repository> {
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
            |url| project_id_of(&url),
        );
    Some(Repository { id, git_dir })
}

/// Follows a `.git` pointer file as written. `commondir` is followed the same way; only `HEAD`, `config` and `commondir` are read there.
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
/// The host is lowercased and a port is dropped. `host:8443/owner/repo` without a scheme is scp-style, not a port.
fn project_id_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unzip();
    let without_extras = rest.unwrap_or(url).split(['?', '#']).next().unwrap_or_default();
    let host_and_path = without_extras.rsplit('@').next().unwrap_or_default();
    let id = if scheme.is_none() && host_and_path.starts_with(['/', '.']) {
        host_and_path.to_string()
    } else {
        let (host, after_host) = split_host(host_and_path);
        let path = if scheme.is_some() {
            after_host.split_once('/').map_or("", |(_port, path)| path)
        } else {
            after_host.strip_prefix([':', '/']).unwrap_or_default()
        };
        format!("{}/{path}", host.to_lowercase())
    };
    id.trim_end_matches('/').trim_end_matches(".git").to_string()
}

/// Splits `host:port/path`, `host:path` and `[::1]:path` into the host and what follows it.
fn split_host(host_and_path: &str) -> (&str, &str) {
    let host_end = if host_and_path.starts_with('[') {
        host_and_path.find(']').map_or(host_and_path.len(), |close| close + 1)
    } else {
        host_and_path.find([':', '/']).unwrap_or(host_and_path.len())
    };
    host_and_path.split_at(host_end)
}

type Locate = Box<dyn Fn(&Path) -> Option<Repository> + Send + Sync>;

struct Entry {
    repository: Option<Repository>,
    found_at: Instant,
}

/// Remembers which repository each directory belongs to, so its `config` is read once rather than once per request.
/// The branch is not remembered: a checkout changes it, so every lookup reads `HEAD` again.
/// A directory in no repository is looked up again after `negative_ttl`, since `git init` may follow.
pub struct ProjectCache {
    found: Mutex<HashMap<PathBuf, Entry>>,
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

#[cfg(test)]
mod tests;
