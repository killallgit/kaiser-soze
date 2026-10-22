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

fn resolved(dir: &Path) -> Option<Project> {
    ProjectCache::default().project_for(dir)
}

#[test]
fn a_repository_is_its_origin_remote_on_its_branch() {
    // It gives host/owner/repo and the checked-out branch
    let dir = tempfile::tempdir().unwrap();
    on_main(dir.path(), Some("https://example.com/owner/repo.git"));

    assert_eq!(resolved(dir.path()), project("example.com/owner/repo", Some("main")));
}

#[test]
fn a_subdirectory_resolves_to_the_repository_above_it() {
    // It walks up to the nearest .git
    let dir = tempfile::tempdir().unwrap();
    on_main(dir.path(), Some("https://example.com/owner/repo.git"));
    let nested = dir.path().join("src/deep");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(resolved(&nested), project("example.com/owner/repo", Some("main")));
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

        let found = resolved(dir.path());

        assert_eq!(found, project("example.com/owner/repo", Some("main")), "{remote}");
        for secret in ["pass", "pa/ss", "token", "abc", "frag"] {
            assert!(!found.as_ref().unwrap().id.contains(secret), "{remote}");
        }
    }
}

#[test]
fn every_form_of_one_remote_gives_one_id() {
    // It lowercases the host, drops the port and keeps the path as written
    let remotes = [
        "ssh://git@GitHub.com:2222/o/r.git",
        "git@github.com:o/r.git",
        "https://github.com/o/r",
        "https://github.com:8443/o/r",
        "ssh://git@github.com:22/o/r/",
    ];
    for remote in remotes {
        assert_eq!(project_id_of(remote), "github.com/o/r", "{remote}");
    }
    assert_eq!(project_id_of("https://github.com/Owner/Repo"), "github.com/Owner/Repo");
    assert_eq!(project_id_of("git@github.com:8443/o/r"), "github.com/8443/o/r");
}

#[test]
fn a_bracketed_ipv6_host_stays_whole() {
    // It keeps the address, drops the port, and reads the scp colon after the bracket
    let remotes = [
        "https://[::1]:8443/o/r",
        "ssh://git@[::1]:22/o/r.git",
        "git@[::1]:o/r.git",
        "https://[::1]/o/r",
    ];
    for remote in remotes {
        assert_eq!(project_id_of(remote), "[::1]/o/r", "{remote}");
    }
}

#[test]
fn no_remote_gives_the_resolved_path() {
    // It names the repository by where it is
    let dir = tempfile::tempdir().unwrap();
    on_main(dir.path(), None);

    assert_eq!(
        resolved(dir.path()),
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

    assert_eq!(resolved(&linked), project("example.com/owner/repo", Some("feature")));
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

    let found = resolved(&linked).unwrap();

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

    assert_eq!(resolved(dir.path()), project("example.com/owner/repo", None));
}

#[test]
fn a_directory_in_no_repository_gives_none() {
    // It finds no project
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(resolved(dir.path()), None);
}

#[test]
fn a_missing_directory_gives_none() {
    // It finds no project, even under a repository
    let dir = tempfile::tempdir().unwrap();
    on_main(dir.path(), Some("https://example.com/owner/repo"));

    assert_eq!(resolved(&dir.path().join("gone")), None);
}

#[test]
fn the_cache_keeps_the_repository_but_reads_the_branch_again() {
    // It answers the id from the cache and the branch from the current HEAD
    let dir = tempfile::tempdir().unwrap();
    repository(
        dir.path(),
        "ref: refs/heads/a\n",
        Some("https://example.com/owner/repo"),
    );
    let cache = ProjectCache::default();
    cache.project_for(dir.path());

    write(&dir.path().join(".git/HEAD"), "ref: refs/heads/b\n");
    fs::remove_file(dir.path().join(".git/config")).unwrap();

    assert_eq!(
        cache.project_for(dir.path()),
        project("example.com/owner/repo", Some("b"))
    );
}

#[test]
fn a_cached_repository_on_a_detached_head_gives_no_branch() {
    // It keeps the project and drops the branch once HEAD stops naming one
    let dir = tempfile::tempdir().unwrap();
    on_main(dir.path(), Some("https://example.com/owner/repo"));
    let cache = ProjectCache::default();
    cache.project_for(dir.path());

    write(
        &dir.path().join(".git/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    );

    assert_eq!(cache.project_for(dir.path()), project("example.com/owner/repo", None));
}

#[test]
fn the_cache_remembers_a_directory_in_no_repository_within_the_ttl() {
    // It does not look again while the negative answer is fresh
    let dir = tempfile::tempdir().unwrap();
    let cache = ProjectCache::default();
    assert_eq!(cache.project_for(dir.path()), None);

    on_main(dir.path(), Some("https://example.com/owner/repo"));

    assert_eq!(cache.project_for(dir.path()), None);
}

#[test]
fn a_directory_in_no_repository_is_looked_up_again_after_the_ttl() {
    // It finds a repository created after the negative answer expired
    let dir = tempfile::tempdir().unwrap();
    let cache = ProjectCache::default().with_negative_ttl(Duration::ZERO);
    assert_eq!(cache.project_for(dir.path()), None);

    on_main(dir.path(), Some("https://example.com/owner/repo"));

    assert_eq!(
        cache.project_for(dir.path()),
        project("example.com/owner/repo", Some("main"))
    );
}

#[test]
fn a_found_repository_never_expires() {
    // It keeps answering from the cache even with a zero negative ttl
    let dir = tempfile::tempdir().unwrap();
    on_main(dir.path(), Some("https://example.com/owner/repo"));
    let cache = ProjectCache::default().with_negative_ttl(Duration::ZERO);
    cache.project_for(dir.path());

    fs::remove_file(dir.path().join(".git/config")).unwrap();

    assert_eq!(
        cache.project_for(dir.path()),
        project("example.com/owner/repo", Some("main"))
    );
}

#[test]
fn looking_a_known_directory_up_again_keeps_the_other_entries() {
    // It replaces the entry in place instead of clearing a full cache
    let dir = tempfile::tempdir().unwrap();
    let cache = ProjectCache::default().with_negative_ttl(Duration::ZERO);
    for n in 0..CACHE_CAPACITY {
        cache.project_for(&dir.path().join(n.to_string()));
    }

    cache.project_for(&dir.path().join("0"));

    assert_eq!(cache.found.lock().unwrap().len(), CACHE_CAPACITY);
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
