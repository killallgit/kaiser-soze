use crate::adapter::{claims_host, Adapter, RequestHead, ResponseHead, TapInput};
use crate::ca::Ca;
use crate::exchange::{ExchangeTap, Exchanges, Opening, Side, TeeBody};
use crate::label::{self, Label};
use crate::live::Live;
use crate::project::{ProjectCache, ProjectLookups};
use crate::redact::redact_headers;
use crate::shape::ShapeLog;
use crate::sink::Sink;
use crate::BoxError;
use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper::header::{ACCEPT_ENCODING, UPGRADE};
use hyper::service::service_fn;
use hyper::upgrade::OnUpgrade;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use serde_json::json;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const ACCEPT_RETRY: Duration = Duration::from_millis(50);

type Out = UnsyncBoxBody<Bytes, hyper::Error>;
type Upstream = SendRequest<TeeBody<Incoming>>;

pub struct ProxyConfig {
    pub adapters: Vec<Arc<dyn Adapter>>,
    pub ca_dir: PathBuf,
    pub tap_file: PathBuf,
    /// Where to write the shape of each distinct request and reply, or none.
    pub shape_file: Option<PathBuf>,
    pub upstream_roots: RootCertStore,
    pub upstream_addr: Option<SocketAddr>,
    /// Shows a short line per event on a terminal, or none.
    pub live: Option<Live>,
}

pub struct Proxy {
    adapters: Vec<Arc<dyn Adapter>>,
    ca: Ca,
    connector: TlsConnector,
    upstream_addr: Option<SocketAddr>,
    live: bool,
    exchanges: Exchanges,
}

struct Tunnel {
    proxy: Arc<Proxy>,
    adapter: Arc<dyn Adapter>,
    host: String,
    port: u16,
    label: Option<Label>,
    upstream: tokio::sync::Mutex<Upstream>,
}

impl Proxy {
    pub async fn new(config: ProxyConfig) -> Result<Arc<Self>, BoxError> {
        Self::with_projects(config, ProjectCache::default()).await
    }

    /// Like `new`, finding the project of each connection's directory with `projects`.
    pub async fn with_projects(config: ProxyConfig, projects: ProjectCache) -> Result<Arc<Self>, BoxError> {
        let hosts: Vec<String> = config
            .adapters
            .iter()
            .flat_map(|adapter| adapter.hosts().to_vec())
            .collect();
        let mut client = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(config.upstream_roots)
            .with_no_client_auth();
        client.alpn_protocols = vec![b"http/1.1".to_vec()];
        let shapes = match &config.shape_file {
            Some(path) => Some(ShapeLog::new(Sink::open(path).await?)),
            None => None,
        };
        Ok(Arc::new(Self {
            ca: Ca::load_or_create(&config.ca_dir, &hosts)?,
            adapters: config.adapters,
            connector: TlsConnector::from(Arc::new(client)),
            upstream_addr: config.upstream_addr,
            live: config.live.is_some(),
            exchanges: Exchanges::new(
                Sink::open_with(&config.tap_file, config.live).await?,
                shapes,
                ProjectLookups::new(projects),
            ),
        }))
    }

    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                tokio::time::sleep(ACCEPT_RETRY).await;
                continue;
            };
            let proxy = self.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| proxy.clone().handle_connect(req));
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    }

    async fn handle_connect(self: Arc<Self>, req: Request<Incoming>) -> Result<Response<Out>, Infallible> {
        let status = match self.open_tunnel(req).await {
            Ok(()) => StatusCode::OK,
            Err(_) => StatusCode::BAD_GATEWAY,
        };
        Ok(empty_response(status))
    }

    async fn dial(&self, host: &str, port: u16) -> std::io::Result<TcpStream> {
        let target = self.upstream_addr.map_or(format!("{host}:{port}"), |a| a.to_string());
        TcpStream::connect(target).await
    }

    async fn connect_upstream(&self, host: &str, port: u16) -> Result<Upstream, BoxError> {
        let tcp = self.dial(host, port).await?;
        let tls = self
            .connector
            .connect(ServerName::try_from(host.to_string())?, tcp)
            .await?;
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await?;
        tokio::spawn(connection.with_upgrades());
        Ok(sender)
    }

    async fn open_tunnel(self: &Arc<Self>, req: Request<Incoming>) -> Result<(), BoxError> {
        if req.method() != Method::CONNECT {
            return Err("only CONNECT is supported".into());
        }
        let authority = req.uri().authority().ok_or("CONNECT without a host")?;
        let (host, port) = (authority.host().to_string(), authority.port_u16().unwrap_or(443));
        let label = label::from_headers(req.headers());
        if !self.live {
            eprintln!("connect {host}:{port}");
        }
        let Some(adapter) = self.adapters.iter().find(|a| claims_host(a.as_ref(), &host)) else {
            self.exchanges
                .note_alone("none", "tunnel", json!({"host": host, "port": port}), label.as_ref());
            let upstream = self.dial(&host, port).await?;
            let upgrade = hyper::upgrade::on(req);
            tokio::spawn(async move {
                if let Ok(client) = upgrade.await {
                    let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(client), &mut { upstream }).await;
                }
            });
            return Ok(());
        };
        let sender = self.connect_upstream(&host, port).await?;
        let upgrade = hyper::upgrade::on(req);
        let acceptor = TlsAcceptor::from(self.ca.server_config_for(&host)?);
        let tunnel = Arc::new(Tunnel {
            proxy: self.clone(),
            adapter: adapter.clone(),
            host,
            port,
            label,
            upstream: tokio::sync::Mutex::new(sender),
        });
        tokio::spawn(async move {
            let Ok(client) = upgrade.await else { return };
            let Ok(tls) = acceptor.accept(TokioIo::new(client)).await else {
                return;
            };
            let service = service_fn(move |req| tunnel.clone().forward(req));
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(tls), service)
                .with_upgrades()
                .await;
        });
        Ok(())
    }
}

impl Tunnel {
    async fn send_upstream(&self, req: Request<TeeBody<Incoming>>) -> Result<Response<Incoming>, BoxError> {
        let mut upstream = self.upstream.lock().await;
        if upstream.ready().await.is_err() {
            *upstream = self.proxy.connect_upstream(&self.host, self.port).await?;
        }
        Ok(upstream.send_request(req).await?)
    }

    async fn forward(self: Arc<Self>, mut req: Request<Incoming>) -> Result<Response<Out>, Infallible> {
        let head = request_head(&self.host, &req);
        let tap = self.tap_for(&head, &req);
        let client_upgrade = req
            .headers()
            .contains_key(UPGRADE)
            .then(|| hyper::upgrade::on(&mut req));
        let (mut parts, body) = req.into_parts();
        if tap.is_some() {
            parts.headers.remove(ACCEPT_ENCODING);
        }
        let req = Request::from_parts(parts, TeeBody::new(body, tap.clone(), Side::Request));
        let sent = self.send_upstream(req).await;
        let Ok(mut res) = sent else {
            return Ok(empty_response(StatusCode::BAD_GATEWAY));
        };
        if let Some(tap) = &tap {
            note_response_head(tap, &res);
        }
        if res.status() == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_upgrade) = client_upgrade {
                tokio::spawn(bridge(client_upgrade, hyper::upgrade::on(&mut res), tap));
            }
            return Ok(res.map(|body| body.boxed_unsync()));
        }
        Ok(res.map(|body| TeeBody::new(body, tap, Side::Response).boxed_unsync()))
    }

    fn tap_for(&self, head: &RequestHead, req: &Request<Incoming>) -> Option<ExchangeTap> {
        let exchanges = &self.proxy.exchanges;
        let Some(parser) = self.adapter.claim(head) else {
            let path = head.path.split('?').next().unwrap_or_default();
            let event = crate::common::unsupported(&head.host, path);
            exchanges.note_alone(self.adapter.name(), &event.kind, event.payload, self.label.as_ref());
            return None;
        };
        Some(exchanges.begin(Opening {
            adapter: self.adapter.as_ref(),
            head,
            parser,
            label: self.label.as_ref(),
            http_version: req.version(),
            upgrade: req.headers().contains_key(UPGRADE),
        }))
    }
}

fn request_head(host: &str, req: &Request<Incoming>) -> RequestHead {
    RequestHead {
        method: req.method().to_string(),
        host: host.to_string(),
        path: req.uri().path().to_string(),
        headers: redact_headers(req.headers()),
    }
}

fn note_response_head(tap: &ExchangeTap, res: &Response<Incoming>) {
    let headers = redact_headers(res.headers());
    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let transport = if res.status() == StatusCode::SWITCHING_PROTOCOLS {
        "websocket"
    } else if content_type.contains("event-stream") {
        "sse"
    } else {
        "http"
    };
    tap.stats.response_head_arrived();
    tap.note(
        "response_head",
        json!({
            "status": res.status().as_u16(),
            "http_version": format!("{:?}", res.version()),
            "transport": transport,
            "headers": headers,
        }),
    );
    tap.send(TapInput::ResponseHead(ResponseHead {
        status: res.status().as_u16(),
        headers,
    }));
}

fn empty_response(status: StatusCode) -> Response<Out> {
    let mut res = Response::new(Empty::new().map_err(|never| match never {}).boxed_unsync());
    *res.status_mut() = status;
    res
}

async fn bridge(client: OnUpgrade, server: OnUpgrade, tap: Option<ExchangeTap>) {
    let (Ok(client), Ok(server)) = (client.await, server.await) else {
        return;
    };
    let (client_read, client_write) = tokio::io::split(TokioIo::new(client));
    let (server_read, server_write) = tokio::io::split(TokioIo::new(server));
    tokio::join!(
        pump(client_read, server_write, tap.clone(), true),
        pump(server_read, client_write, tap, false)
    );
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut from: R,
    mut to: W,
    tap: Option<ExchangeTap>,
    from_client: bool,
) {
    let mut buf = vec![0u8; 16 * 1024];
    while let Ok(n @ 1..) = from.read(&mut buf).await {
        let received_at = Instant::now();
        if to.write_all(&buf[..n]).await.is_err() || to.flush().await.is_err() {
            break;
        }
        if let Some(tap) = &tap {
            tap.stats.chunk_handed_on(received_at.elapsed());
            if !from_client {
                tap.stats.response_chunk_arrived(n);
            }
            tap.send_ws_bytes(from_client, Bytes::copy_from_slice(&buf[..n]));
        }
    }
    let _ = to.shutdown().await;
}
