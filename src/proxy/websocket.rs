//! Upgrade completion and byte-preserving bidirectional WebSocket relaying.

use super::Out;
use crate::exchange::ExchangeTap;
use crate::logging::{Fields, Level, Logger};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use hyper::Response;
use hyper_util::rt::TokioIo;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) fn relay(
    client: Option<OnUpgrade>,
    mut response: Response<Incoming>,
    tap: Option<ExchangeTap>,
    log: Logger,
) -> Response<Out> {
    if let Some(client) = client {
        tokio::spawn(bridge(client, hyper::upgrade::on(&mut response), tap, log));
    }
    response.map(|body| body.boxed_unsync())
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
            let handoff = received_at.elapsed();
            tap.send_ws_bytes(context.from_client, Bytes::copy_from_slice(&buf[..n]), handoff);
        }
    }
    let _ = to.shutdown().await;
}
