use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use kaiser_soze::adapter::Adapter;
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
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
const SECRET: &str = "secret-value-xyz";
const EVENT_GAP: Duration = Duration::from_millis(200);

fn codex() -> Arc<dyn Adapter> {
    Arc::new(CodexAdapter::new(vec![CLAIMED.to_string()]))
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn sse_events() -> Vec<String> {
    let mut events: Vec<String> = ["a", "b", "c", "d"]
        .iter()
        .map(|d| format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{d}\"}}\n\n"))
        .collect();
    events.push(
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"usage\":{\"output_tokens\":4}}}\n\n"
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
    let leaf = CertificateParams::new(vec![CLAIMED.to_string(), UNCLAIMED.to_string()])
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
    tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    for event in events {
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
    let proxy = Proxy::new(ProxyConfig {
        adapters: vec![adapter],
        ca_dir: dir.join("ca"),
        tap_file: dir.join("tap.jsonl"),
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
    let body = r#"{"model":"m1","input":[{},{}],"tools":[{"type":"function","name":"shell"}]}"#;
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
    while let Some(frame) = response.body_mut().frame().await {
        if let Some(data) = frame.unwrap().data_ref() {
            first_chunk_at.get_or_insert_with(Instant::now);
            received.extend_from_slice(data);
        }
    }

    assert_eq!(status, 200);
    assert_eq!(received, sse_events().concat().into_bytes());
    let finished_at = upstream.finished_at.lock().unwrap().expect("upstream finished");
    assert!(first_chunk_at.unwrap() < finished_at);
    let events = read_events(&dir.path().join("tap.jsonl")).await;
    assert_eq!(count(&events, "response.output_text.delta"), 4);
    assert_eq!(count(&events, "response.completed"), 1);
    let request = events.iter().find(|e| e["event"] == "request").unwrap();
    assert_eq!(request["payload"]["model"], "m1");
    assert_eq!(request["payload"]["input_items"], 2);
    assert_eq!(request["payload"]["tools"][0], "shell");
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
    let body = r#"{"model":"m1","stream":true,"max_tokens":8,"system":"s","messages":[{}],"tools":[{"name":"Bash"}]}"#;
    let request = Request::post("/v1/messages")
        .header("anthropic-beta", "beta-a")
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body)))
        .unwrap();

    let response = client.send_request(request).await.unwrap();
    let received = response.into_body().collect().await.unwrap().to_bytes();

    assert_eq!(received, events.concat().as_bytes());
    let tapped = read_events(&dir.path().join("tap.jsonl")).await;
    assert_eq!(count(&tapped, "content_block_delta"), 3);
    assert_eq!(count(&tapped, "message_stop"), 1);
    let request = tapped.iter().find(|e| e["event"] == "request").unwrap();
    assert_eq!(request["adapter"], "claude");
    assert_eq!(request["payload"]["model"], "m1");
    assert_eq!(request["payload"]["tools"][0], "Bash");
    assert_eq!(request["payload"]["anthropic_beta"], "beta-a");
    let raw = std::fs::read_to_string(dir.path().join("tap.jsonl")).unwrap();
    assert!(!raw.contains(SECRET));
}
