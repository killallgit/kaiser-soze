use crate::support::*;
use http_body_util::BodyExt;

const LABEL_CLAUDE: &str = "Basic Y2xhdWRlOi9Vc2Vycy9tZS9w"; // claude:/Users/me/p
const LABEL_AGENT: &str = "Basic YWdlbnQ6L1VzZXJzL21lL3A="; // agent:/Users/me/p

#[tokio::test]
async fn a_connection_label_reaches_exchange_start_and_unsupported() {
    // The label from Proxy-Authorization is metadata on that connection's lines
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;
    let (status, tunnel) = connect_tunnel_as(proxy, CLAUDE_HOST, Some(LABEL_CLAUDE)).await;
    assert_eq!(status, 200);
    let mut client = http_client(tunnel, CLAUDE_HOST, roots).await;

    for request in [
        claude_request("/v1/messages", CLAUDE_BODY),
        claude_request("/v1/messages/count_tokens", CLAUDE_BODY),
    ] {
        let response = client.send_request(request).await.unwrap();
        response.into_body().collect().await.unwrap();
    }

    let tap_file = dir.path().join("tap.jsonl");
    wait_for_line(&tap_file, "unsupported").await;
    let lines = tap_lines(&tap_file);
    let label = serde_json::json!({"command": "claude", "dir": "/Users/me/p"});
    let start = lines.iter().find(|line| line["event"] == "exchange_start").unwrap();
    assert_eq!(start["payload"]["label"], label);
    let unsupported = lines.iter().find(|line| line["event"] == "unsupported").unwrap();
    assert_eq!(unsupported["payload"]["label"], label);
}

#[tokio::test]
async fn bad_or_missing_credentials_still_forward_and_carry_no_label() {
    // A label is never a reason to refuse or change a connection
    for auth in [None, Some("Basic !!!"), Some("Bearer x"), Some("Basic bm9jb2xvbg==")] {
        let upstream = start_upstream(anthropic_events()).await;
        let dir = tempfile::tempdir().unwrap();
        let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;
        let (status, tunnel) = connect_tunnel_as(proxy, CLAUDE_HOST, auth).await;
        assert_eq!(status, 200);
        let mut client = http_client(tunnel, CLAUDE_HOST, roots).await;

        let response = client
            .send_request(claude_request("/v1/messages", CLAUDE_BODY))
            .await
            .unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(body, anthropic_events().concat().as_bytes());
        let tap_file = dir.path().join("tap.jsonl");
        wait_for_line(&tap_file, "exchange_start").await;
        let lines = tap_lines(&tap_file);
        let start = lines.iter().find(|line| line["event"] == "exchange_start").unwrap();
        assert!(start["payload"].get("label").is_none());
    }
}

#[tokio::test]
async fn an_unclaimed_host_writes_a_tunnel_event_with_the_label() {
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, _tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;

    let (status, _tunnel) = connect_tunnel_as(proxy, UNCLAIMED, Some(LABEL_AGENT)).await;

    assert_eq!(status, 200);
    let tap_file = dir.path().join("tap.jsonl");
    wait_for_line(&tap_file, "tunnel").await;
    let lines = tap_lines(&tap_file);
    assert_eq!(
        lines[0]["payload"],
        serde_json::json!({"host": UNCLAIMED, "port": 443, "label": {"command": "agent", "dir": "/Users/me/p"}})
    );
}
