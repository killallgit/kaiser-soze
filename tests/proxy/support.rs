use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use kaiser_soze::adapter::{Adapter, TapEvent};
use kaiser_soze::assembler::Assembler;
use kaiser_soze::claude::ClaudeAdapter;
use kaiser_soze::codex::CodexAdapter;
use kaiser_soze::live::Live;
use kaiser_soze::project::ProjectCache;
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

pub const CLAIMED: &str = "fake.test";
pub const UNCLAIMED: &str = "other.test";
pub const CLAUDE_HOST: &str = "claude.test";
pub const SECRET: &str = "secret-value-xyz";
pub const EVENT_GAP: Duration = Duration::from_millis(200);

pub fn codex() -> Arc<dyn Adapter> {
    Arc::new(CodexAdapter::new(vec![CLAIMED.to_string()]))
}

pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn sse_events() -> Vec<String> {
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

pub struct FakeUpstream {
    pub addr: SocketAddr,
    pub roots: RootCertStore,
    pub finished_at: Arc<Mutex<Option<Instant>>>,
    pub heads: Arc<Mutex<Vec<String>>>,
}

pub async fn start_upstream(events: Vec<String>) -> FakeUpstream {
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

pub async fn serve_sse<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
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

pub async fn start_proxy(
    upstream: &FakeUpstream,
    dir: &Path,
    adapter: Arc<dyn Adapter>,
) -> (SocketAddr, RootCertStore) {
    start_proxy_with(upstream, dir, vec![adapter]).await
}

pub async fn start_proxy_with(
    upstream: &FakeUpstream,
    dir: &Path,
    adapters: Vec<Arc<dyn Adapter>>,
) -> (SocketAddr, RootCertStore) {
    start_proxy_live(upstream, dir, adapters, None).await
}

pub async fn start_proxy_live(
    upstream: &FakeUpstream,
    dir: &Path,
    adapters: Vec<Arc<dyn Adapter>>,
    live: Option<Live>,
) -> (SocketAddr, RootCertStore) {
    start_proxy_finding_projects(upstream, dir, adapters, live, ProjectCache::default()).await
}

pub async fn start_proxy_finding_projects(
    upstream: &FakeUpstream,
    dir: &Path,
    adapters: Vec<Arc<dyn Adapter>>,
    live: Option<Live>,
    projects: ProjectCache,
) -> (SocketAddr, RootCertStore) {
    let proxy = Proxy::with_projects(
        ProxyConfig {
            live,
            adapters,
            ca_dir: dir.join("ca"),
            tap_file: dir.join("tap.jsonl"),
            shape_file: Some(dir.join("shapes.jsonl")),
            upstream_roots: upstream.roots.clone(),
            upstream_addr: Some(upstream.addr),
        },
        projects,
    )
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

pub async fn connect_tunnel(proxy: SocketAddr, host: &str) -> (u16, TcpStream) {
    connect_tunnel_as(proxy, host, None).await
}

/// Like `connect_tunnel`, with a `Proxy-Authorization` header value when given.
pub async fn connect_tunnel_as(proxy: SocketAddr, host: &str, auth: Option<&str>) -> (u16, TcpStream) {
    let mut stream = TcpStream::connect(proxy).await.unwrap();
    let auth = auth
        .map(|value| format!("Proxy-Authorization: {value}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n{auth}\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
    }
    let status = String::from_utf8_lossy(&head[9..12]).parse().unwrap();
    (status, stream)
}

pub async fn http_client(tunnel: TcpStream, host: &str, trusted: RootCertStore) -> SendRequest<Full<Bytes>> {
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

pub fn post_request(host: &str) -> Request<Full<Bytes>> {
    let body = r#"{"model":"m1","instructions":"sys","input":[{"role":"user","content":"hi"}],"tools":[{"type":"function","name":"shell"}]}"#;
    Request::post("/backend-api/codex/responses")
        .header("host", host)
        .header("authorization", format!("Bearer {SECRET}"))
        .header("accept-encoding", "gzip, zstd")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

pub async fn read_events(tap_file: &Path) -> Vec<Value> {
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

pub fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["event"] == kind).count()
}

pub fn tap_lines(tap_file: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(tap_file).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub async fn wait_for_line(tap_file: &Path, kind: &str) {
    for _ in 0..20 {
        if count(&tap_lines(tap_file), kind) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no {kind} line within 1 s");
}

pub fn fold(lines: &[Value], exchange: u64) -> Vec<Value> {
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

pub fn records_of(lines: &[Value], exchange: u64) -> Vec<Value> {
    lines
        .iter()
        .filter(|line| line["exchange"] == exchange && line["event"] == "record")
        .map(|line| line["payload"].clone())
        .collect()
}

pub async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

pub async fn fetch(client: &mut SendRequest<Full<Bytes>>, host: &str) -> (StatusCode, Bytes) {
    let response = client.send_request(post_request(host)).await.unwrap();
    let status = response.status();
    (status, response.into_body().collect().await.unwrap().to_bytes())
}

pub fn anthropic_events() -> Vec<String> {
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

pub fn claude() -> Arc<dyn Adapter> {
    Arc::new(ClaudeAdapter::new(vec![CLAUDE_HOST.to_string()]))
}

pub fn claude_request(path: &str, body: &str) -> Request<Full<Bytes>> {
    Request::post(path)
        .header("host", CLAUDE_HOST)
        .header("x-api-key", SECRET)
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

pub const CLAUDE_BODY: &str = r#"{"model":"m1","stream":true,"system":"s","messages":[{"role":"user","content":"hi"}],"tools":[{"name":"Bash"}]}"#;

pub async fn send_through(proxy: SocketAddr, roots: RootCertStore, host: &str, request: Request<Full<Bytes>>) -> Bytes {
    let (_, tunnel) = connect_tunnel(proxy, host).await;
    let mut client = http_client(tunnel, host, roots).await;
    let response = client.send_request(request).await.unwrap();
    response.into_body().collect().await.unwrap().to_bytes()
}

pub fn basic_label(command: &str, dir: &Path) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let text = format!("{command}:{}", dir.display());
    let mut encoded = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            encoded.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
        encoded.push_str(&"=".repeat(3 - chunk.len()));
    }
    format!("Basic {encoded}")
}
