//! CONNECT validation, routing, and passthrough or intercepted tunnel startup.

mod label;

use super::http::Tunnel;
use super::{empty_response, Out, Proxy};
use crate::adapter::{claims_host, Adapter};
use crate::logging::{Fields, Level};
use crate::note::{self, Label, Note};
use crate::BoxError;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

struct Connect {
    host: String,
    port: u16,
    label: Option<Label>,
}

impl Proxy {
    pub(super) async fn handle_connect(self: Arc<Self>, req: Request<Incoming>) -> Result<Response<Out>, Infallible> {
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

    async fn open_tunnel(self: &Arc<Self>, req: Request<Incoming>) -> Result<(), BoxError> {
        if req.method() != Method::CONNECT {
            return Err("only CONNECT is supported".into());
        }
        let authority = req.uri().authority().ok_or("CONNECT without a host")?;
        let connect = Connect {
            host: authority.host().to_string(),
            port: authority.port_u16().unwrap_or(443),
            label: label::from_headers(req.headers()),
        };
        self.logging.event(
            Level::Debug,
            "connection_opened",
            Fields {
                port: Some(connect.port),
                ..Fields::default()
            },
        );
        match self.adapters.iter().find(|a| claims_host(a.as_ref(), &connect.host)) {
            Some(adapter) => connect.intercept(self, req, adapter.clone()).await,
            None => connect.passthrough(self, req).await,
        }
    }
}

impl Connect {
    async fn passthrough(self, proxy: &Proxy, req: Request<Incoming>) -> Result<(), BoxError> {
        let tunnel = note::Tunnel {
            host: self.host.clone(),
            label: self.label,
            port: self.port,
        };
        proxy.exchanges.note_alone("none", Note::Tunnel(tunnel));
        let upstream = proxy.upstream.dial(&self.host, self.port).await?;
        let upgrade = hyper::upgrade::on(req);
        let log = proxy.logging.clone();
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
        Ok(())
    }

    async fn intercept(
        self,
        proxy: &Arc<Proxy>,
        req: Request<Incoming>,
        adapter: Arc<dyn Adapter>,
    ) -> Result<(), BoxError> {
        let upstream = proxy
            .upstream
            .connect(&self.host, self.port, proxy.logging.clone())
            .await?;
        let upgrade = hyper::upgrade::on(req);
        let acceptor = TlsAcceptor::from(proxy.ca.server_config_for(&self.host)?);
        let tunnel = Arc::new(Tunnel {
            proxy: proxy.clone(),
            adapter,
            host: self.host,
            label: self.label,
            upstream,
        });
        let log = proxy.logging.clone();
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
