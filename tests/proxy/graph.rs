use crate::support::*;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::SendRequest;
use hyper::Request;
use kaiser_soze::project::ProjectCache;
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn repository_with_origin(root: &Path) {
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(
        root.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://user:token@example.com/owner/repo.git\n",
    )
    .unwrap();
}

fn identified_request(headers: &[(&str, &str)]) -> Request<Full<Bytes>> {
    let mut request = claude_request("/v1/messages", CLAUDE_BODY);
    for (name, value) in headers {
        request.headers_mut().insert(
            hyper::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    request
}

fn edges_of(lines: &[Value], exchange: u64) -> Vec<&Value> {
    lines
        .iter()
        .filter(|line| line["exchange"] == exchange && line["event"] == "edge")
        .map(|line| &line["payload"])
        .collect()
}

#[tokio::test]
async fn identity_headers_and_a_labelled_repository_write_edges_after_exchange_start() {
    // It writes in_project, in_session, in_thread and spawned, and the record carries the ids
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let repository = dir.path().join("repo");
    repository_with_origin(&repository);
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;
    let label = basic_label("claude", &repository);
    let (_, tunnel) = connect_tunnel_as(proxy, CLAUDE_HOST, Some(&label)).await;
    let mut client = http_client(tunnel, CLAUDE_HOST, roots).await;
    let request = identified_request(&[
        ("x-claude-code-session-id", "s1"),
        ("x-claude-code-agent-id", "a2"),
        ("x-claude-code-parent-agent-id", "a1"),
    ]);

    let response = client.send_request(request).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(body, anthropic_events().concat().as_bytes());
    let lines = read_events(&dir.path().join("tap.jsonl")).await;
    let edges = edges_of(&lines, 1);
    let relations: Vec<(&str, &str, &str)> = edges
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap(),
                e["from"].as_str().unwrap(),
                e["to"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        relations,
        [
            ("in_project", "session:s1", "project:example.com/owner/repo"),
            ("in_session", "thread:a2", "session:s1"),
            ("in_thread", "exchange:1", "thread:a2"),
            ("spawned", "thread:a2", "thread:a1"),
        ]
    );
    assert_eq!(edges[0]["branch"], "main");
    let position = |kind: &str| lines.iter().position(|line| line["event"] == kind).unwrap();
    assert!(position("exchange_start") < position("edge"));
    let record = &records_of(&lines, 1)[0];
    assert_eq!(record["project"], "example.com/owner/repo");
    assert_eq!(record["session"], "s1");
    assert_eq!(record["thread"], "a2");
    let raw = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(!raw.contains("token"));
}

#[tokio::test]
async fn a_request_without_identity_headers_writes_no_edges_and_forwards_the_same_bytes() {
    // It taps as before: exchange_start, no edge, no ids on the record
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAUDE_HOST).await;
    let mut client = http_client(tunnel, CLAUDE_HOST, roots).await;

    let response = client.send_request(identified_request(&[])).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(body, anthropic_events().concat().as_bytes());
    let lines = read_events(&dir.path().join("tap.jsonl")).await;
    assert_eq!(count(&lines, "exchange_start"), 1);
    assert_eq!(count(&lines, "edge"), 0);
    let record = &records_of(&lines, 1)[0];
    for key in ["project", "session", "thread"] {
        assert!(record.get(key).is_none());
    }
}

#[tokio::test]
async fn a_stalled_project_lookup_does_not_hold_the_exchange_open() {
    // It ends the exchange, writes the edges that need no project, and leaves project off the record
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (release, stalled) = std::sync::mpsc::channel::<()>();
    let stalled = Mutex::new(stalled);
    let projects = ProjectCache::locating_with(move |_| {
        let _ = stalled.lock().unwrap().recv();
        None
    });
    let (proxy, roots) = start_proxy_finding_projects(&upstream, dir.path(), vec![claude()], projects).await;
    let label = basic_label("claude", dir.path());
    let (_, tunnel) = connect_tunnel_as(proxy, CLAUDE_HOST, Some(&label)).await;
    let mut client = http_client(tunnel, CLAUDE_HOST, roots).await;
    let request = identified_request(&[("x-claude-code-session-id", "s1"), ("x-claude-code-agent-id", "a2")]);

    let response = client.send_request(request).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    drop(client);

    assert_eq!(body, anthropic_events().concat().as_bytes());
    let lines = read_events(&dir.path().join("tap.jsonl")).await;
    let kinds: Vec<&str> = edges_of(&lines, 1)
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["in_session", "in_thread"]);
    assert!(records_of(&lines, 1)[0].get("project").is_none());
    drop(release);
}

fn exchange_ended(tap_file: &Path, exchange: u64) -> bool {
    tap_lines(tap_file)
        .iter()
        .any(|line| line["exchange"] == exchange && line["event"] == "exchange_end")
}

struct StalledLookups {
    client: SendRequest<Full<Bytes>>,
    tap_file: std::path::PathBuf,
    calls: Arc<AtomicUsize>,
    stalling: Option<std::sync::mpsc::Sender<()>>,
    exchanges: u64,
    _dir: tempfile::TempDir,
}

impl StalledLookups {
    async fn start() -> Self {
        let upstream = start_upstream(anthropic_events()).await;
        let dir = tempfile::tempdir().unwrap();
        let (release, stalled) = std::sync::mpsc::channel::<()>();
        let stalled = Mutex::new(stalled);
        let calls = Arc::new(AtomicUsize::new(0));
        let projects = ProjectCache::locating_with({
            let calls = calls.clone();
            move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                let _ = stalled.lock().unwrap().recv();
                None
            }
        })
        .with_negative_ttl(Duration::ZERO);
        let (proxy, roots) = start_proxy_finding_projects(&upstream, dir.path(), vec![claude()], projects).await;
        let label = basic_label("claude", dir.path());
        let (_, tunnel) = connect_tunnel_as(proxy, CLAUDE_HOST, Some(&label)).await;
        Self {
            client: http_client(tunnel, CLAUDE_HOST, roots).await,
            tap_file: dir.path().join("tap.jsonl"),
            calls,
            stalling: Some(release),
            exchanges: 0,
            _dir: dir,
        }
    }

    async fn exchange(&mut self) -> u64 {
        let request = identified_request(&[("x-claude-code-session-id", "s1")]);
        self.client
            .send_request(request)
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap();
        self.exchanges += 1;
        let exchange = self.exchanges;
        wait_until(|| exchange_ended(&self.tap_file, exchange)).await;
        exchange
    }

    async fn with_the_cap_reached() -> Self {
        let mut stalled = Self::start().await;
        for _ in 0..MAX_STALLED {
            stalled.exchange().await;
        }
        wait_until(|| stalled.calls() == MAX_STALLED).await;
        stalled
    }

    fn release(&mut self) {
        self.stalling = None;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

const MAX_STALLED: usize = 4;

#[tokio::test]
async fn no_fifth_project_lookup_starts_while_four_hang() {
    // It skips the lookup, so the exchange ends without a project but with its other edges
    // (in_session, in_thread, and continues from the stalled exchanges before it)
    let mut stalled = StalledLookups::with_the_cap_reached().await;

    let exchange = stalled.exchange().await;

    assert_eq!(stalled.calls(), MAX_STALLED);
    let lines = tap_lines(&stalled.tap_file);
    assert!(records_of(&lines, exchange)[0].get("project").is_none());
    assert_eq!(edges_of(&lines, exchange).len(), 3);
}

#[tokio::test]
async fn project_lookups_start_again_once_the_stalled_ones_end() {
    // It gives the permits back, so a later exchange does its lookup
    let mut stalled = StalledLookups::with_the_cap_reached().await;
    stalled.exchange().await;

    stalled.release();
    for _ in 0..MAX_STALLED * 5 {
        if stalled.calls() > MAX_STALLED {
            break;
        }
        stalled.exchange().await;
    }

    assert_eq!(stalled.calls(), MAX_STALLED + 1);
}
