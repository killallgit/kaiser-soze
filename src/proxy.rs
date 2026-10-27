mod connect;
mod http;
mod upstream;
mod websocket;

use crate::adapter::Adapter;
use crate::ca::Ca;
use crate::exchange::{Exchanges, TapOutput};
use crate::live::Live;
use crate::logging::{Fields, Level, Logger};
use crate::project::{ProjectCache, ProjectLookups};
use crate::shape::ShapeLog;
use crate::sink::Sink;
use crate::BoxError;
use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::RootCertStore;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

const ACCEPT_RETRY: Duration = Duration::from_millis(50);

type Out = UnsyncBoxBody<Bytes, hyper::Error>;

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
    upstream: Arc<upstream::Upstream>,
    logging: Logger,
    exchanges: Exchanges,
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
        let upstream = upstream::Upstream::new(config.upstream_roots, config.upstream_addr)?;
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
            upstream: Arc::new(upstream),
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
}

fn empty_response(status: StatusCode) -> Response<Out> {
    let mut res = Response::new(Empty::new().map_err(|never| match never {}).boxed_unsync());
    *res.status_mut() = status;
    res
}
