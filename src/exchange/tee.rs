use super::tap::ExchangeTap;
use crate::adapter::TapInput;
use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
pub enum Side {
    Request,
    Response,
}

pub struct TeeBody<B> {
    inner: B,
    tap: Option<ExchangeTap>,
    side: Side,
    received_at: Option<Instant>,
    ended: bool,
}

impl<B> TeeBody<B> {
    pub fn new(inner: B, tap: Option<ExchangeTap>, side: Side) -> Self {
        Self {
            inner,
            tap,
            side,
            received_at: None,
            ended: false,
        }
    }
}

impl<B: Body<Data = Bytes> + Unpin> Body for TeeBody<B> {
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        let Some(tap) = &this.tap else {
            return polled;
        };
        if let Some(received_at) = this.received_at.take() {
            tap.handed_on(received_at.elapsed());
        }
        if let Poll::Ready(Some(Ok(frame))) = &polled {
            if let Some(data) = frame.data_ref() {
                this.received_at = Some(Instant::now());
                tap.send_body(this.side, data.clone());
            }
        }
        // hyper stops polling once a body reports its end, so the end is noted here too
        let finished = matches!(polled, Poll::Ready(None)) || this.inner.is_end_stream();
        if finished && !this.ended {
            this.ended = true;
            tap.send(match this.side {
                Side::Request => TapInput::RequestEnd,
                Side::Response => TapInput::ResponseEnd,
            });
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
