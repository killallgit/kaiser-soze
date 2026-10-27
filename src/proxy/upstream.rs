//! Upstream TCP/TLS/HTTP connections and serialized requests with readiness-based reconnects.

use crate::exchange::TeeBody;
use crate::logging::{Fields, Level, Logger};
use crate::BoxError;
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;

type Sender = SendRequest<TeeBody<Incoming>>;

pub(super) struct Upstream {
    connector: TlsConnector,
    addr: Option<SocketAddr>,
}

pub(super) struct Connection {
    upstream: Arc<Upstream>,
    host: String,
    port: u16,
    sender: Mutex<Sender>,
    logging: Logger,
}

impl Upstream {
    pub(super) fn new(roots: RootCertStore, addr: Option<SocketAddr>) -> Result<Self, BoxError> {
        let mut client = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            connector: TlsConnector::from(Arc::new(client)),
            addr,
        })
    }

    pub(super) async fn dial(&self, host: &str, port: u16) -> std::io::Result<TcpStream> {
        let target = self.addr.map_or(format!("{host}:{port}"), |a| a.to_string());
        TcpStream::connect(target).await
    }

    pub(super) async fn connect(
        self: &Arc<Self>,
        host: &str,
        port: u16,
        logging: Logger,
    ) -> Result<Connection, BoxError> {
        Ok(Connection {
            upstream: self.clone(),
            host: host.to_string(),
            port,
            sender: Mutex::new(self.handshake(host, port, &logging).await?),
            logging,
        })
    }

    async fn handshake(&self, host: &str, port: u16, logging: &Logger) -> Result<Sender, BoxError> {
        let tcp = self.dial(host, port).await.inspect_err(|error| {
            logging.event(Level::Warn, "upstream_connect_failed", Fields::io(error));
        })?;
        let tls = self
            .connector
            .connect(ServerName::try_from(host.to_string())?, tcp)
            .await
            .inspect_err(|_| {
                logging.event(Level::Warn, "upstream_tls_failed", Fields::default());
            })?;
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .inspect_err(|_| {
                logging.event(Level::Warn, "upstream_http_failed", Fields::default());
            })?;
        let log = logging.clone();
        tokio::spawn(async move {
            if connection.with_upgrades().await.is_err() {
                log.event(Level::Debug, "upstream_connection_closed", Fields::default());
            }
        });
        Ok(sender)
    }
}

impl Connection {
    pub(super) async fn send(&self, req: Request<TeeBody<Incoming>>) -> Result<Response<Incoming>, BoxError> {
        let mut sender = self.sender.lock().await;
        if sender.ready().await.is_err() {
            *sender = self.upstream.handshake(&self.host, self.port, &self.logging).await?;
        }
        Ok(sender.send_request(req).await?)
    }
}
