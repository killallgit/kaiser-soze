use super::id::project_id_of;
use super::Project;
use std::fs;
use std::path::{Path, PathBuf};

/// A repository found for a directory: its stable id, and where its own `HEAD` lives.
#[derive(Clone, Debug)]
pub struct Repository {
    pub(super) id: String,
    git_dir: PathBuf,
}

impl Repository {
    pub(super) fn project(&self) -> Project {
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
