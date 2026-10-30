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
use std::io;
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

fn read_failure_level(from_client: bool, error: &io::Error) -> Level {
    let peer_closed = matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    );
    if from_client && peer_closed {
        Level::Debug
    } else {
        Level::Warn
    }
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut from: R, mut to: W, context: Pump) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) => {
                let level = read_failure_level(context.from_client, &error);
                context.log.event(level, "stream_read_failed", Fields::io(&error));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::{Format, Settings};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    struct FailingRead(io::ErrorKind);

    impl AsyncRead for FailingRead {
        fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::from(self.0)))
        }
    }

    async fn logged_level(from_client: bool, kind: io::ErrorKind) -> String {
        let (writer, mut reader) = tokio::io::duplex(8192);
        let settings = Settings {
            level: Level::Debug,
            format: Format::Json,
            ..Settings::default()
        };
        let log = Logger::new(writer, settings);
        let context = Pump {
            tap: None,
            from_client,
            log: log.clone(),
        };
        pump(FailingRead(kind), tokio::io::sink(), context).await;
        drop(log);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(record["event"], "stream_read_failed");
        record["level"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn a_client_closing_the_tunnel_is_logged_below_warn() {
        for kind in [
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::BrokenPipe,
        ] {
            assert_eq!(logged_level(true, kind).await, "debug");
        }
    }

    #[tokio::test]
    async fn a_client_read_fault_is_still_a_warning() {
        assert_eq!(logged_level(true, io::ErrorKind::PermissionDenied).await, "warn");
    }

    #[tokio::test]
    async fn an_upstream_read_failure_is_a_warning_even_when_it_looks_like_a_close() {
        assert_eq!(logged_level(false, io::ErrorKind::UnexpectedEof).await, "warn");
        assert_eq!(logged_level(false, io::ErrorKind::ConnectionReset).await, "warn");
    }
}
