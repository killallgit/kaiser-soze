use super::clock::{Clock, Stamp};
use super::recorder::Recorder;
use super::stats::Stats;
use super::ws::WsDecoder;
use crate::adapter::{ExchangeParser, ResponseHead, TapInput};
use crate::assembler::Assembler;
use crate::capture::headers::redact_headers;
use crate::line::Entry;
use crate::note::{self, Note, Transport};
use bytes::Bytes;
use hyper::{Response, StatusCode};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const TAP_QUEUE: usize = 256;

enum Msg {
    Input(TapInput),
    WsBytes { from_client: bool, bytes: Bytes },
}

#[derive(Clone)]
pub struct ExchangeTap {
    tx: mpsc::Sender<(Stamp, Msg)>,
    clock: Clock,
    recorder: Recorder,
    stats: Arc<Stats>,
}

impl ExchangeTap {
    pub fn exchange_id(&self) -> u64 {
        self.recorder.exchange
    }
    pub(crate) fn spawn(recorder: Recorder, clock: Clock, parser: Box<dyn ExchangeParser>) -> Self {
        let (tx, rx) = mpsc::channel(TAP_QUEUE);
        let stats = Arc::new(Stats::new());
        tokio::spawn(
            TapTask {
                rx,
                parser,
                recorder: recorder.clone(),
                clock,
                stats: stats.clone(),
            }
            .run(),
        );
        Self {
            tx,
            clock,
            recorder,
            stats,
        }
    }

    pub(super) fn note(&self, note: Note) {
        self.recorder.write(self.clock.stamp(), Entry::Note(note), (0, 0));
    }

    /// Writes the `response_head` line and hands the head to the parser.
    pub fn note_response_head<B>(&self, res: &Response<B>) {
        let headers = redact_headers(res.headers());
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let transport = if res.status() == StatusCode::SWITCHING_PROTOCOLS {
            Transport::Websocket
        } else if content_type.contains("event-stream") {
            Transport::Sse
        } else {
            Transport::Http
        };
        self.stats.response_head_arrived();
        self.note(Note::ResponseHead(note::ResponseHead {
            headers: headers.clone(),
            http_version: format!("{:?}", res.version()),
            status: res.status().as_u16(),
            transport,
        }));
        self.send(TapInput::ResponseHead(ResponseHead {
            status: res.status().as_u16(),
            headers,
        }));
    }

    pub fn send(&self, input: TapInput) {
        self.offer(Msg::Input(input));
    }

    pub(super) fn send_body(&self, side: super::Side, bytes: Bytes) {
        if side == super::Side::Response {
            self.stats.response_chunk_arrived(bytes.len());
        }
        self.send(match side {
            super::Side::Request => TapInput::RequestBody(bytes),
            super::Side::Response => TapInput::ResponseBody(bytes),
        });
    }

    pub fn send_ws_bytes(&self, from_client: bool, bytes: Bytes, handoff: Duration) {
        self.handed_on(handoff);
        if !from_client {
            self.stats.response_chunk_arrived(bytes.len());
        }
        self.offer(Msg::WsBytes { from_client, bytes });
    }

    pub(super) fn handed_on(&self, elapsed: Duration) {
        self.stats.chunk_handed_on(elapsed);
    }

    fn offer(&self, msg: Msg) {
        if let Err(refused) = self.tx.try_send((self.clock.stamp(), msg)) {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            if let (_, Msg::WsBytes { from_client, .. }) = refused.into_inner() {
                self.stats.ws_desynced[from_client as usize].store(true, Ordering::Relaxed);
            }
        }
    }
}

struct TapTask {
    rx: mpsc::Receiver<(Stamp, Msg)>,
    parser: Box<dyn ExchangeParser>,
    recorder: Recorder,
    clock: Clock,
    stats: Arc<Stats>,
}

impl TapTask {
    async fn run(self) {
        let Self {
            mut rx,
            mut parser,
            recorder,
            clock,
            stats,
        } = self;
        let mut decoders = [WsDecoder::default(), WsDecoder::default()];
        let mut assembler = Assembler::default();
        let dropped = || stats.dropped.load(Ordering::Relaxed) + recorder.refused();
        while let Some((stamp, msg)) = rx.recv().await {
            let inputs = match msg {
                Msg::Input(input) => vec![input],
                Msg::WsBytes { from_client, bytes } => {
                    let decoder = &mut decoders[from_client as usize];
                    if stats.ws_desynced[from_client as usize].load(Ordering::Relaxed) {
                        decoder.mark_broken();
                    }
                    decoder
                        .push(&bytes)
                        .into_iter()
                        .map(|m| TapInput::WsMessage {
                            from_client,
                            text: m.text,
                            start: m.start,
                            end: m.end,
                        })
                        .collect()
                }
            };
            for event in inputs.into_iter().flat_map(|input| parser.feed(input)) {
                let records = assembler.push(&event.event, dropped());
                recorder.write(stamp, Entry::Event(event.event), (event.start, event.end));
                for record in records {
                    recorder.write_record(stamp, record).await;
                }
            }
        }
        if let Some(record) = assembler.finish(dropped()) {
            recorder.write_record(clock.stamp(), record).await;
        }
        let end = Note::ExchangeEnd(stats.summary(recorder.output.sink.dropped()));
        recorder.write_reliable(clock.stamp(), Entry::Note(end)).await;
    }
}
