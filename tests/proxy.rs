use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use kaiser_soze::adapter::{Adapter, ExchangeParser, RequestHead, TapEvent, TapInput};
use kaiser_soze::assembler::Assembler;
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::common::*;
use kaiser_soze::proxy::{Proxy, ProxyConfig};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const CLAIMED: &str = "fake.test";
const UNCLAIMED: &str = "other.test";
const CLAUDE_HOST: &str = "claude.test";
const SECRET: &str = "secret-value-xyz";
const EVENT_GAP: Duration = Duration::from_millis(200);

fn codex() -> Arc<dyn Adapter> {
    Arc::new(CodexAdapter::new(vec![CLAIMED.to_string()]))
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn sse_events() -> Vec<String> {
    let mut events = vec![
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\"}}\n\n"
            .to_string(),
    ];
    events.extend(["a", "b", "c", "d"].iter().map(|d| {
        format!("data: {{\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"{d}\"}}\n\n")
    }));
    events.push(
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"output_tokens\":4}}}\n\n"
            .into(),
    );
    events
}

struct FakeUpstream {
    addr: SocketAddr,
    roots: RootCertStore,
    finished_at: Arc<Mutex<Option<Instant>>>,
    heads: Arc<Mutex<Vec<String>>>,
}

async fn start_upstream(events: Vec<String>) -> FakeUpstream {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec![
        CLAIMED.to_string(),
        UNCLAIMED.to_string(),
        CLAUDE_HOST.to_string(),
    ])
    .unwrap()
    .signed_by(&leaf_key, &Issuer::from_params(&ca_params, &ca_key))
    .unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let finished_at = Arc::new(Mutex::new(None));
    let finished = finished_at.clone();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen_heads = heads.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let finished = finished.clone();
            let seen_heads = seen_heads.clone();
            let events = events.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                if let Some(head) = serve_sse(&mut tls, &events).await {
                    seen_heads.lock().unwrap().push(head);
                }
                drop(tls);
                *finished.lock().unwrap() = Some(Instant::now());
            });
        }
    });
    let mut roots = RootCertStore::empty();
    roots.add(ca_cert.der().clone()).unwrap();
    FakeUpstream {
        addr,
        roots,
        finished_at,
        heads,
    }
}

async fn serve_sse<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    tls: &mut S,
    events: &[String],
) -> Option<String> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        if tls.read_exact(&mut byte).await.is_err() {
            return None;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    let body_len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .map_or(0, |n| n.trim().parse().unwrap());
    let mut body = vec![0u8; body_len];
    tls.read_exact(&mut body).await.unwrap();
    let events = if head.contains("/v1/messages") {
        anthropic_events()
    } else {
        events.to_vec()
    };
    tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    for event in &events {
        tls.write_all(format!("{:x}\r\n{event}\r\n", event.len()).as_bytes())
            .await
            .unwrap();
        tls.flush().await.unwrap();
        tokio::time::sleep(EVENT_GAP).await;
    }
    tls.write_all(b"0\r\n\r\n").await.unwrap();
    tls.flush().await.unwrap();
    Some(head)
}

async fn start_proxy(upstream: &FakeUpstream, dir: &Path, adapter: Arc<dyn Adapter>) -> (SocketAddr, RootCertStore) {
    start_proxy_with(upstream, dir, vec![adapter]).await
}

async fn start_proxy_with(
    upstream: &FakeUpstream,
    dir: &Path,
    adapters: Vec<Arc<dyn Adapter>>,
) -> (SocketAddr, RootCertStore) {
    let proxy = Proxy::new(ProxyConfig {
        adapters,
        ca_dir: dir.join("ca"),
        tap_file: dir.join("tap.jsonl"),
        shape_file: Some(dir.join("shapes.jsonl")),
        upstream_roots: upstream.roots.clone(),
        upstream_addr: Some(upstream.addr),
    })
    .await
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(proxy.serve(listener));
    let pem = std::fs::read(dir.join("ca/ca.pem")).unwrap();
    let mut tap_roots = RootCertStore::empty();
    tap_roots.add(CertificateDer::from_pem_slice(&pem).unwrap()).unwrap();
    (addr, tap_roots)
}

async fn connect_tunnel(proxy: SocketAddr, host: &str) -> (u16, TcpStream) {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    stream
        .write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
    }
    let status = String::from_utf8_lossy(&head[9..12]).parse().unwrap();
    (status, stream)
}

async fn http_client(tunnel: TcpStream, host: &str, trusted: RootCertStore) -> SendRequest<Full<Bytes>> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(trusted)
        .with_no_client_auth();
    let tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from(host.to_string()).unwrap(), tunnel)
        .await
        .unwrap();
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await.unwrap();
    tokio::spawn(connection);
    sender
}

fn post_request(host: &str) -> Request<Full<Bytes>> {
    let body = r#"{"model":"m1","instructions":"sys","input":[{"role":"user","content":"hi"}],"tools":[{"type":"function","name":"shell"}]}"#;
    Request::post("/backend-api/codex/responses")
        .header("host", host)
        .header("authorization", format!("Bearer {SECRET}"))
        .header("accept-encoding", "gzip, zstd")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

async fn read_events(tap_file: &Path) -> Vec<Value> {
    for _ in 0..50 {
        let text = std::fs::read_to_string(tap_file).unwrap_or_default();
        let events: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        if events.iter().any(|e| e["event"] == "exchange_end") {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tap file never held an exchange_end");
}

fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["event"] == kind).count()
}

fn tap_lines(tap_file: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(tap_file).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

async fn wait_for_line(tap_file: &Path, kind: &str) {
    for _ in 0..20 {
        if count(&tap_lines(tap_file), kind) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no {kind} line within 1 s");
}

fn fold(lines: &[Value], exchange: u64) -> Vec<Value> {
    let mut assembler = Assembler::default();
    let mut records: Vec<Value> = lines
        .iter()
        .filter(|line| line["exchange"] == exchange)
        .flat_map(|line| {
            let event = TapEvent {
                kind: line["event"].as_str().unwrap().to_string(),
                payload: line["payload"].clone(),
                start: 0,
                end: 0,
            };
            assembler.push(&event, 0)
        })
        .collect();
    records.extend(assembler.finish(0));
    records
}

fn records_of(lines: &[Value], exchange: u64) -> Vec<Value> {
    lines
        .iter()
        .filter(|line| line["exchange"] == exchange && line["event"] == "record")
        .map(|line| line["payload"].clone())
        .collect()
}

#[tokio::test]
async fn relays_a_claimed_sse_stream_live_and_taps_it() {
    // It forwards each event as it arrives, byte-identical, and records parsed events
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (status, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;

    let mut response = client.send_request(post_request(CLAIMED)).await.unwrap();
    let mut received = Vec::new();
    let mut first_chunk_at = None;
    let mut checked_live = false;
    let tap_file = dir.path().join("tap.jsonl");
    while let Some(frame) = response.body_mut().frame().await {
        if let Some(data) = frame.unwrap().data_ref() {
            first_chunk_at.get_or_insert_with(Instant::now);
            received.extend_from_slice(data);
            if !checked_live && String::from_utf8_lossy(&received).contains("\"delta\":\"a\"") {
                checked_live = true;
                wait_for_line(&tap_file, "block_delta").await;
                assert_eq!(count(&tap_lines(&tap_file), "record"), 0);
            }
        }
    }

    assert_eq!(status, 200);
    assert_eq!(received, sse_events().concat().into_bytes());
    let finished_at = upstream.finished_at.lock().unwrap().expect("upstream finished");
    assert!(first_chunk_at.unwrap() < finished_at);
    let events = read_events(&dir.path().join("tap.jsonl")).await;
    assert!(checked_live);
    assert_eq!(count(&events, "block_delta"), 4);
    assert_eq!(count(&events, "usage"), 1);
    assert_eq!(count(&events, "reply_end"), 1);
    let records = records_of(&events, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["incomplete"], false);
    assert_eq!(records[0]["conversation"]["system"], "sys");
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "abcd");
    assert_eq!(fold(&events, 1), records);
    let raw = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(!raw.contains(SECRET));
    let end = events.iter().find(|e| e["event"] == "exchange_end").unwrap();
    assert_eq!(end["payload"]["sink_dropped"], 0);
    let heads = upstream.heads.lock().unwrap();
    assert!(heads.iter().all(|head| !head.contains("accept-encoding")));
}

async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

async fn fetch(client: &mut SendRequest<Full<Bytes>>, host: &str) -> (StatusCode, Bytes) {
    let response = client.send_request(post_request(host)).await.unwrap();
    let status = response.status();
    (status, response.into_body().collect().await.unwrap().to_bytes())
}

#[tokio::test]
async fn redials_upstream_when_it_closed_between_requests() {
    // It answers a second request on the same tunnel after the upstream hung up
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    fetch(&mut client, CLAIMED).await;
    wait_until(|| upstream.finished_at.lock().unwrap().is_some()).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (status, body) = fetch(&mut client, CLAIMED).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, sse_events().concat().as_bytes());
}

#[tokio::test]
async fn tunnels_an_unclaimed_host_without_decrypting() {
    // It lets the client see the upstream's own certificate, and taps nothing
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, _tap_roots) = start_proxy(&upstream, dir.path(), codex()).await;
    let (_, tunnel) = connect_tunnel(proxy, UNCLAIMED).await;
    let mut client = http_client(tunnel, UNCLAIMED, upstream.roots.clone()).await;

    let response = client.send_request(post_request(UNCLAIMED)).await.unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(body, sse_events().concat().as_bytes());
    let tap = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(tap.is_empty());
}

#[tokio::test]
async fn refuses_the_tunnel_when_upstream_is_unreachable() {
    // It answers CONNECT with a failure status instead of 200
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    let upstream = FakeUpstream {
        addr: dead_addr,
        roots: RootCertStore::empty(),
        finished_at: Arc::default(),
        heads: Arc::default(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (proxy, _) = start_proxy(&upstream, dir.path(), codex()).await;

    let (status, _) = connect_tunnel(proxy, CLAIMED).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY.as_u16());
}

fn anthropic_events() -> Vec<String> {
    let block = |name: &str, data: &str| format!("event: {name}\ndata: {data}\n\n");
    let mut events = vec![
        block(
            "message_start",
            r#"{"type":"message_start","message":{"id":"m","model":"m1","usage":{"input_tokens":5}}}"#,
        ),
        block(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
    ];
    for word in ["a", "b", "c"] {
        events.push(block(
            "content_block_delta",
            &format!(r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{word}"}}}}"#),
        ));
    }
    events.push(block(
        "content_block_stop",
        r#"{"type":"content_block_stop","index":0}"#,
    ));
    events.push(block(
        "message_delta",
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
    ));
    events.push(block("message_stop", r#"{"type":"message_stop"}"#));
    events
}

#[tokio::test]
async fn taps_an_anthropic_stream_through_the_claude_adapter() {
    // It relays the stream unchanged and records the request summary and every delta
    let events = anthropic_events();
    let upstream = start_upstream(events.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let adapter: Arc<dyn Adapter> = Arc::new(ClaudeAdapter::new(vec![CLAIMED.to_string()]));
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), adapter).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    let body = r#"{"model":"m1","stream":true,"max_tokens":8,"system":"s","messages":[{"role":"user","content":"hi"}],"tools":[{"name":"Bash"}]}"#;
    let request = Request::post("/v1/messages")
        .header("anthropic-beta", "beta-a")
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    let received = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(received, events.concat().as_bytes());
    let tapped = read_events(&dir.path().join("tap.jsonl")).await;
    assert_eq!(count(&tapped, "block_delta"), 3);
    assert_eq!(count(&tapped, "reply_end"), 1);
    let conversation = tapped.iter().find(|e| e["event"] == "conversation").unwrap();
    assert_eq!(conversation["adapter"], "claude");
    assert_eq!(conversation["payload"]["tools"][0]["name"], "Bash");
    let records = records_of(&tapped, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "abc");
    assert_eq!(records[0]["reply"]["usage"]["input"], 5);
    assert_eq!(records[0]["reply"]["usage"]["output"], 3);
    assert_eq!(fold(&tapped, 1), records);
    let raw = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(!raw.contains(SECRET));
}

#[tokio::test]
async fn writes_the_shape_of_each_request_and_event_without_their_values() {
    // It logs keys and types of the request and every distinct event, and no header or body value
    let upstream = start_upstream(anthropic_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let adapter: Arc<dyn Adapter> = Arc::new(ClaudeAdapter::new(vec![CLAIMED.to_string()]));
    let (proxy, tap_roots) = start_proxy(&upstream, dir.path(), adapter).await;
    let (_, tunnel) = connect_tunnel(proxy, CLAIMED).await;
    let mut client = http_client(tunnel, CLAIMED, tap_roots).await;
    let body =
        r#"{"model":"m1","stream":true,"max_tokens":8,"system":"s","messages":[{"role":"user","content":"hi"}]}"#;
    let request = Request::post("/v1/messages")
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    response.into_body().collect().await.unwrap();

    let shapes_file = dir.path().join("shapes.jsonl");
    wait_until(|| {
        std::fs::read_to_string(&shapes_file)
            .unwrap_or_default()
            .contains("message_stop")
    })
    .await;
    let lines = tap_lines(&shapes_file);
    let kinds: Vec<&str> = lines.iter().filter_map(|l| l["kind"].as_str()).collect();
    assert!(kinds.contains(&"message_start"));
    assert_eq!(kinds.iter().filter(|k| **k == "content_block_delta").count(), 1);
    let request_shape = lines.iter().find(|l| l["direction"] == "request").unwrap();
    assert_eq!(request_shape["shape"]["messages"][0]["role"], "string=user");
    assert_eq!(request_shape["shape"]["model"], "string");
    let raw = std::fs::read_to_string(&shapes_file).unwrap();
    assert!(!raw.contains(SECRET) && !raw.contains("\"hi\""));
}

fn claude() -> Arc<dyn Adapter> {
    Arc::new(ClaudeAdapter::new(vec![CLAUDE_HOST.to_string()]))
}

fn claude_request(path: &str, body: &str) -> Request<Full<Bytes>> {
    Request::post(path)
        .header("host", CLAUDE_HOST)
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

const CLAUDE_BODY: &str = r#"{"model":"m1","stream":true,"system":"s","messages":[{"role":"user","content":"hi"}],"tools":[{"name":"Bash"}]}"#;

async fn send_through(proxy: SocketAddr, roots: RootCertStore, host: &str, request: Request<Full<Bytes>>) -> Bytes {
    let (_, tunnel) = connect_tunnel(proxy, host).await;
    let mut client = http_client(tunnel, host, roots).await;
    let response = client.send_request(request).await.unwrap();
    response.into_body().collect().await.unwrap().to_bytes()
}

#[tokio::test]
async fn concurrent_exchanges_of_both_agents_stay_separate_and_share_one_shape() {
    // It records one conversation and one record per request, never mixing exchanges
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy_with(&upstream, dir.path(), vec![codex(), claude()]).await;
    let mut calls = Vec::new();
    for _ in 0..3 {
        calls.push(tokio::spawn(send_through(
            proxy,
            roots.clone(),
            CLAIMED,
            post_request(CLAIMED),
        )));
        calls.push(tokio::spawn(send_through(
            proxy,
            roots.clone(),
            CLAUDE_HOST,
            claude_request("/v1/messages", CLAUDE_BODY),
        )));
    }

    let mut bodies = Vec::new();
    for call in calls {
        bodies.push(call.await.unwrap());
    }
    let tap_file = dir.path().join("tap.jsonl");
    for _ in 0..100 {
        if count(&tap_lines(&tap_file), "exchange_end") == 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let lines = tap_lines(&tap_file);
    let codex_bytes = sse_events().concat().into_bytes();
    let claude_bytes = anthropic_events().concat().into_bytes();
    assert_eq!(bodies.iter().filter(|body| **body == codex_bytes).count(), 3);
    assert_eq!(bodies.iter().filter(|body| **body == claude_bytes).count(), 3);
    assert_eq!(count(&lines, "conversation"), 6);
    assert_eq!(count(&lines, "record"), 6);
    let mut texts = Vec::new();
    for exchange in 1..=6u64 {
        let own: Vec<&Value> = lines.iter().filter(|line| line["exchange"] == exchange).collect();
        assert!(own.iter().all(|line| line["adapter"] == own[0]["adapter"]));
        let records = records_of(&lines, exchange);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["incomplete"], false);
        assert_eq!(fold(&lines, exchange), records);
        let mut fields: Vec<&String> = records[0].as_object().unwrap().keys().collect();
        fields.sort();
        assert_eq!(fields, ["conversation", "dropped", "incomplete", "reply"]);
        texts.push(
            records[0]["reply"]["message"]["blocks"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    texts.sort();
    assert_eq!(texts, ["abc", "abc", "abc", "abcd", "abcd", "abcd"]);
}

#[tokio::test]
async fn a_claimed_host_whose_adapter_declines_is_relayed_untouched_and_reported() {
    // It writes one unsupported line with host and path only, without the query string
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;

    let body = send_through(
        proxy,
        roots,
        CLAUDE_HOST,
        claude_request("/v1/messages/count_tokens?beta=true", CLAUDE_BODY),
    )
    .await;

    assert_eq!(body, anthropic_events().concat().as_bytes());
    let tap_file = dir.path().join("tap.jsonl");
    wait_for_line(&tap_file, "unsupported").await;
    let lines = tap_lines(&tap_file);
    let unsupported = lines.iter().find(|line| line["event"] == "unsupported").unwrap();
    assert_eq!(
        unsupported["payload"],
        serde_json::json!({"host": CLAUDE_HOST, "path": "/v1/messages/count_tokens"})
    );
    assert_eq!(count(&lines, "record"), 0);
}

struct Echo {
    hosts: Vec<String>,
}

struct EchoParser;

impl Adapter for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn hosts(&self) -> &[String] {
        &self.hosts
    }

    fn claim(&self, _head: &RequestHead) -> Option<Box<dyn ExchangeParser>> {
        Some(Box::new(EchoParser))
    }
}

impl ExchangeParser for EchoParser {
    fn feed(&mut self, input: TapInput) -> Vec<TapEvent> {
        if !matches!(input, TapInput::ResponseEnd) {
            return Vec::new();
        }
        vec![
            conversation(Value::Null, Value::Null, vec![message("user", vec![text_block("hi")])]),
            block_start(BlockStart {
                index: 0,
                kind: "text",
                id: Value::Null,
                name: Value::Null,
            }),
            block_delta(0, Delta::Text("echo".into())),
            reply_end(Value::Null),
        ]
    }
}

#[tokio::test]
async fn a_new_adapter_that_emits_common_events_is_captured_without_a_core_change() {
    // It gets live lines and one record from the unchanged proxy
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let echo: Arc<dyn Adapter> = Arc::new(Echo {
        hosts: vec![CLAIMED.to_string()],
    });
    let (proxy, roots) = start_proxy(&upstream, dir.path(), echo).await;

    send_through(proxy, roots, CLAIMED, post_request(CLAIMED)).await;

    let lines = read_events(&dir.path().join("tap.jsonl")).await;
    let records = records_of(&lines, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["reply"]["message"]["blocks"][0]["text"], "echo");
    assert_eq!(
        lines.iter().find(|line| line["event"] == "record").unwrap()["adapter"],
        "echo"
    );
}

#[tokio::test]
async fn credentials_and_image_bytes_never_reach_the_tap_file() {
    // It keeps both out of every line and leaves only a reference for the image
    let upstream = start_upstream(sse_events()).await;
    let dir = tempfile::tempdir().unwrap();
    let (proxy, roots) = start_proxy(&upstream, dir.path(), claude()).await;
    let body = r#"{"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"SU1HQllURVNNQVJLRVI="}}]}]}"#;

    send_through(proxy, roots, CLAUDE_HOST, claude_request("/v1/messages", body)).await;

    let tap_file = dir.path().join("tap.jsonl");
    let lines = read_events(&tap_file).await;
    let raw = std::fs::read_to_string(&tap_file).unwrap();
    assert_eq!(raw.matches(SECRET).count(), 0);
    assert_eq!(raw.matches("SU1HQllURVNNQVJLRVI").count(), 0);
    let conversation = lines.iter().find(|line| line["event"] == "conversation").unwrap();
    assert_eq!(
        conversation["payload"]["messages"][0]["blocks"][0],
        image_block("0:0", "image/png", 14)
    );
}
