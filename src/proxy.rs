use crate::adapter::{claims_host, Adapter, RequestHead};
use crate::ca::Ca;
use crate::exchange::{ExchangeTap, Exchanges, Opening, Side, TapOutput, TeeBody};
use crate::label::{self, Label};
use crate::live::Live;
use crate::logging::{Fields, Level, Logger};
use crate::note::{self, Note};
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
    pub logging: Logger,
}

pub struct Proxy {
    adapters: Vec<Arc<dyn Adapter>>,
    ca: Ca,
    connector: TlsConnector,
    upstream_addr: Option<SocketAddr>,
    logging: Logger,
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
        let output = TapOutput::open(&config.tap_file, config.live, config.logging).await?;
        let logging = output.logging.component("proxy");
        let shapes = match &config.shape_file {
            Some(path) => Some(ShapeLog::new(
                Sink::open_reporting(path, logging.component("shape")).await?,
            )),
            None => None,
        };
        let ca = Ca::load_or_create(&config.ca_dir, &hosts).inspect_err(|_| {
            logging.event(Level::Error, "ca_initialization_failed", Fields::default());
        })?;
        Ok(Arc::new(Self {
            ca,
            adapters: config.adapters,
            connector: TlsConnector::from(Arc::new(client)),
            upstream_addr: config.upstream_addr,
            logging,
            exchanges: Exchanges::new(output, shapes, ProjectLookups::new(projects)),
        }))
    }

    pub fn logger(&self) -> Logger {
        self.logging.clone()
    }

    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(error) => {
                    self.logging.event(Level::Warn, "accept_failed", Fields::io(&error));
                    tokio::time::sleep(ACCEPT_RETRY).await;
                    continue;
                }
            };
            let proxy = self.clone();
            tokio::spawn(async move {
                let log = proxy.logging.clone();
                let service = service_fn(move |req| proxy.clone().handle_connect(req));
                if hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await
                    .is_err()
                {
                    log.event(Level::Debug, "client_http_failed", Fields::default());
                }
            });
        }
    }

    async fn handle_connect(self: Arc<Self>, req: Request<Incoming>) -> Result<Response<Out>, Infallible> {
        let status = match self.open_tunnel(req).await {
            Ok(()) => StatusCode::OK,
            Err(_) => {
                self.logging.event(
                    Level::Warn,
                    "connect_failed",
                    Fields {
                        status: Some(502),
                        ..Fields::default()
                    },
                );
                StatusCode::BAD_GATEWAY
            }
        };
        Ok(empty_response(status))
    }

    async fn dial(&self, host: &str, port: u16) -> std::io::Result<TcpStream> {
        let target = self.upstream_addr.map_or(format!("{host}:{port}"), |a| a.to_string());
        TcpStream::connect(target).await
    }

    async fn connect_upstream(&self, host: &str, port: u16) -> Result<Upstream, BoxError> {
        let tcp = self.dial(host, port).await.inspect_err(|error| {
            self.logging
                .event(Level::Warn, "upstream_connect_failed", Fields::io(error));
        })?;
        let tls = self
            .connector
            .connect(ServerName::try_from(host.to_string())?, tcp)
            .await
            .inspect_err(|_| {
                self.logging
                    .event(Level::Warn, "upstream_tls_failed", Fields::default());
            })?;
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .inspect_err(|_| {
                self.logging
                    .event(Level::Warn, "upstream_http_failed", Fields::default());
            })?;
        let log = self.logging.clone();
        tokio::spawn(async move {
            if connection.with_upgrades().await.is_err() {
                log.event(Level::Debug, "upstream_connection_closed", Fields::default());
            }
        });
        Ok(sender)
    }

    async fn open_tunnel(self: &Arc<Self>, req: Request<Incoming>) -> Result<(), BoxError> {
        if req.method() != Method::CONNECT {
            return Err("only CONNECT is supported".into());
        }
        let authority = req.uri().authority().ok_or("CONNECT without a host")?;
        let (host, port) = (authority.host().to_string(), authority.port_u16().unwrap_or(443));
        let label = label::from_headers(req.headers());
        self.logging.event(
            Level::Debug,
            "connection_opened",
            Fields {
                port: Some(port),
                ..Fields::default()
            },
        );
        let Some(adapter) = self.adapters.iter().find(|a| claims_host(a.as_ref(), &host)) else {
            let tunnel = note::Tunnel {
                host: host.clone(),
                label,
                port,
            };
            self.exchanges.note_alone("none", Note::Tunnel(tunnel));
            let upstream = self.dial(&host, port).await?;
            let upgrade = hyper::upgrade::on(req);
            let log = self.logging.clone();
            tokio::spawn(async move {
                if let Ok(client) = upgrade.await {
                    if tokio::io::copy_bidirectional(&mut TokioIo::new(client), &mut { upstream })
                        .await
                        .is_err()
                    {
                        log.event(Level::Warn, "tunnel_copy_failed", Fields::default());
                    }
                } else {
                    log.event(Level::Debug, "client_upgrade_failed", Fields::default());
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
        let log = self.logging.clone();
        tokio::spawn(async move {
            let Ok(client) = upgrade.await else {
                log.event(Level::Debug, "client_upgrade_failed", Fields::default());
                return;
            };
            let Ok(tls) = acceptor.accept(TokioIo::new(client)).await else {
                log.event(Level::Warn, "client_tls_failed", Fields::default());
                return;
            };
            let service = service_fn(move |req| tunnel.clone().forward(req));
            if hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(tls), service)
                .with_upgrades()
                .await
                .is_err()
            {
                log.event(Level::Debug, "client_http_failed", Fields::default());
            }
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
            self.proxy.logging.event(
                Level::Warn,
                "upstream_request_failed",
                Fields {
                    exchange: tap.as_ref().map(ExchangeTap::exchange_id),
                    adapter: Some(self.adapter.name()),
                    status: Some(502),
                    ..Fields::default()
                },
            );
            return Ok(empty_response(StatusCode::BAD_GATEWAY));
        };
        if let Some(tap) = &tap {
            tap.note_response_head(&res);
        }
        if res.status() == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(client_upgrade) = client_upgrade {
                tokio::spawn(bridge(
                    client_upgrade,
                    hyper::upgrade::on(&mut res),
                    tap,
                    self.proxy.logging.clone(),
                ));
            }
            return Ok(res.map(|body| body.boxed_unsync()));
        }
        Ok(res.map(|body| TeeBody::new(body, tap, Side::Response).boxed_unsync()))
    }

    fn tap_for(&self, head: &RequestHead, req: &Request<Incoming>) -> Option<ExchangeTap> {
        let exchanges = &self.proxy.exchanges;
        let Some(parser) = self.adapter.claim(head) else {
            let path = head.path.split('?').next().unwrap_or_default();
            let unsupported = note::Unsupported {
                host: head.host.clone(),
                label: self.label.clone(),
                path: path.to_string(),
            };
            exchanges.note_alone(self.adapter.name(), Note::Unsupported(unsupported));
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

fn empty_response(status: StatusCode) -> Response<Out> {
    let mut res = Response::new(Empty::new().map_err(|never| match never {}).boxed_unsync());
    *res.status_mut() = status;
    res
}

async fn bridge(client: OnUpgrade, server: OnUpgrade, tap: Option<ExchangeTap>, log: Logger) {
    let (Ok(client), Ok(server)) = (client.await, server.await) else {
        log.event(Level::Warn, "websocket_upgrade_failed", Fields::default());
        return;
    };
    let (client_read, client_write) = tokio::io::split(TokioIo::new(client));
    let (server_read, server_write) = tokio::io::split(TokioIo::new(server));
    tokio::join!(
        pump(
            client_read,
            server_write,
            Pump {
                tap: tap.clone(),
                from_client: true,
                log: log.clone()
            }
        ),
        pump(
            server_read,
            client_write,
            Pump {
                tap,
                from_client: false,
                log
            }
        )
    );
}

struct Pump {
    tap: Option<ExchangeTap>,
    from_client: bool,
    log: Logger,
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut from: R, mut to: W, context: Pump) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) => {
                context.log.event(Level::Warn, "stream_read_failed", Fields::io(&error));
                break;
            }
        };
        let received_at = Instant::now();
        if let Err(error) = to.write_all(&buf[..n]).await {
            context
                .log
                .event(Level::Warn, "stream_write_failed", Fields::io(&error));
            break;
        }
        if let Err(error) = to.flush().await {
            context
                .log
                .event(Level::Warn, "stream_flush_failed", Fields::io(&error));
            break;
        }
        if let Some(tap) = &context.tap {
            tap.stats.chunk_handed_on(received_at.elapsed());
            if !context.from_client {
                tap.stats.response_chunk_arrived(n);
            }
            tap.send_ws_bytes(context.from_client, Bytes::copy_from_slice(&buf[..n]));
        }
    }
    let _ = to.shutdown().await;
}
