use crate::support::*;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use kaiser_soze::adapter::Adapter;
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::live::Live;
use kaiser_soze::logging::{Format, Level, Logger, Settings};
use kaiser_soze::proxy::{Proxy, ProxyConfig};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;

async fn observing(
    upstream: &FakeUpstream,
    dir: &Path,
    logging: Logger,
) -> (std::net::SocketAddr, rustls::RootCertStore) {
    let proxy = Proxy::new(ProxyConfig {
        adapters: vec![claude()],
        ca_dir: dir.join("ca"),
        tap_file: dir.join("tap.jsonl"),
        shape_file: None,
        upstream_roots: upstream.roots.clone(),
        upstream_addr: Some(upstream.addr),
        live: None,
        logging,
    })
    .await
    .unwrap();
    serve_proxy(proxy, dir).await
}

async fn read_log_until(reader: &mut tokio::io::DuplexStream, event: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            let n = reader.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(record) = bytes
                .split(|b| *b == b'\n')
                .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
                .find(|record| record["event"] == event)
            {
                return record;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn stream_metadata_reaches_the_terminal_before_the_reply_finishes() {
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (writer, mut reader) = tokio::io::duplex(8192);
    let logging = Logger::new(
        writer,
        Settings {
            stream: true,
            level: Level::Debug,
            format: Format::Json,
            ..Settings::default()
        },
    );
    let (proxy, roots) = observing(&upstream, dir.path(), logging).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAUDE_HOST).await;
    let mut client = http_client(tunnel, CLAUDE_HOST, roots).await;
    let response = client
        .send_request(claude_request("/v1/messages", CLAUDE_BODY))
        .await
        .unwrap();
    let receiving = tokio::spawn(async move { response.into_body().collect().await.unwrap().to_bytes() });

    let delta = read_log_until(&mut reader, "block_delta").await;
    assert!(!receiving.is_finished());
    assert!(upstream.finished_at.lock().unwrap().is_none());
    assert_eq!(delta["level"], "debug");
    assert_eq!(delta["adapter"], "claude");
    assert_eq!(delta["exchange"], 1);
    assert_eq!(delta["bytes"], 1);
    assert_eq!(delta["field"], "text");
    assert!(!delta["run_id"].as_str().unwrap().is_empty());
    assert!(delta.get("payload").is_none());
    assert!(delta.get("text").is_none());
    assert_eq!(receiving.await.unwrap(), anthropic_events().concat().as_bytes());
}

#[tokio::test]
async fn a_stalled_terminal_does_not_delay_or_change_forwarded_bytes() {
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (writer, _reader) = tokio::io::duplex(1);
    let logging = Logger::new(
        writer,
        Settings {
            stream: true,
            level: Level::Debug,
            ..Settings::default()
        },
    )
    .with_debug_file(&dir.path().join("debug.jsonl"))
    .await
    .unwrap();
    let (proxy, roots) = observing(&upstream, dir.path(), logging).await;
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        send_through(proxy, roots, CLAUDE_HOST, claude_request("/v1/messages", CLAUDE_BODY)),
    )
    .await
    .unwrap();
    assert_eq!(bytes, anthropic_events().concat().as_bytes());
    let lines = read_events(&dir.path().join("tap.jsonl")).await;
    assert_eq!(count(&lines, "record"), 1);
    assert_eq!(records_of(&lines, 1)[0]["incomplete"], false);
    let debug = dir.path().join("debug.jsonl");
    wait_until(|| {
        std::fs::read_to_string(&debug).is_ok_and(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|record| record["event"] == "record" && record["incomplete"] == false)
        })
    })
    .await;
}

#[tokio::test]
async fn an_upstream_tls_failure_has_a_safe_correlated_diagnostic() {
    let mut upstream = start_upstream(anthropic_events()).await;
    upstream.roots = rustls::RootCertStore::empty();
    let dir = tempfile::tempdir().unwrap();
    let (writer, mut reader) = tokio::io::duplex(8192);
    let logging = Logger::new(
        writer,
        Settings {
            format: Format::Json,
            ..Settings::default()
        },
    );
    let (proxy, _) = observing(&upstream, dir.path(), logging).await;
    let (status, _) = connect_tunnel(proxy, CLAUDE_HOST).await;
    assert_eq!(status, 502);
    let failure = read_log_until(&mut reader, "upstream_tls_failed").await;
    assert_eq!(failure["level"], "warn");
    assert!(!failure["run_id"].as_str().unwrap().is_empty());
    assert!(failure.get("message").is_none());
    assert!(failure.get("headers").is_none());
}

#[tokio::test]
async fn shows_one_request_line_and_one_reply_line_without_message_text() {
    // The live view prints a line per request and per reply, and the tap file is written as usual
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (printed, mut screen) = tokio::io::duplex(8192);
    let live = Live::new(printed, "/home/me".into(), 0);
    let adapters: Vec<Arc<dyn Adapter>> = vec![Arc::new(ClaudeAdapter::new(vec![CLAIMED.to_string()]))];
    let (proxy, tap_roots) = start_proxy_live(&upstream, dir.path(), adapters, Some(live)).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    let body = r#"{"model":"m1","stream":true,"max_tokens":8,"system":"SECRETWORD","messages":[{"role":"user","content":"SECRETWORD"}]}"#;
    let request = Request::post("/v1/messages")
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    response.into_body().collect().await.unwrap();

    let mut seen = String::new();
    let mut chunk = [0u8; 512];
    tokio::time::timeout(Duration::from_secs(5), async {
        while seen.matches('\n').count() < 2 {
            let n = screen.read(&mut chunk).await.unwrap();
            seen.push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    })
    .await
    .unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("-> 1 msgs"));
    assert!(lines[1].contains("<-"));
    assert!(!seen.contains("SECRETWORD"));
    let tap = dir.path().join("tap.jsonl");
    wait_until(|| std::fs::read_to_string(&tap).unwrap_or_default().contains("\"record\"")).await;
}
