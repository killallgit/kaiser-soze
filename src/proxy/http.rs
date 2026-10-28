//! Intercepted HTTP forwarding, tap selection, and response routing.

use super::upstream::Connection;
use super::{empty_response, Out, Proxy};
use crate::adapter::{Adapter, RequestHead};
use crate::capture::headers::redact_headers;
use crate::exchange::{ExchangeTap, Opening, Side, TeeBody};
use crate::logging::{Fields, Level};
use crate::note::{self, Label, Note};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{ACCEPT_ENCODING, UPGRADE};
use hyper::{Request, Response, StatusCode};
use std::convert::Infallible;
use std::sync::Arc;

pub(super) struct Tunnel {
    pub(super) proxy: Arc<Proxy>,
    pub(super) adapter: Arc<dyn Adapter>,
    pub(super) host: String,
    pub(super) label: Option<Label>,
    pub(super) upstream: Connection,
}

impl Tunnel {
    pub(super) async fn forward(self: Arc<Self>, mut req: Request<Incoming>) -> Result<Response<Out>, Infallible> {
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
        let sent = self.upstream.send(req).await;
        let Ok(res) = sent else {
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
            return Ok(super::websocket::relay(
                client_upgrade,
                res,
                tap,
                self.proxy.logging.clone(),
            ));
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
