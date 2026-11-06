use super::clock::{Clock, Stamp};
use super::project_task::Project;
use super::recorder::Recorder;
use super::stats::Stats;
use super::tee::Side;
use super::ws::WsDecoder;
use crate::adapter::{ExchangeParser, ResponseHead, TapInput};
use crate::assembler::Assembler;
use crate::capture::headers::redact_headers;
use crate::graph::Steps;
use crate::line::Entry;
use crate::note::{self, Note, Transport};
use crate::record::Record;
use bytes::Bytes;
use hyper::{Response, StatusCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const TAP_QUEUE: usize = 256;

enum Msg {
    Input(TapInput),
    WsBytes {
        from_client: bool,
        bytes: Bytes,
        after_gap: bool,
    },
}

/// The one door into an exchange's tap. Every byte handed in is counted; after a switch of protocols the
/// bytes of both sides are decoded as WebSocket frames, before it they are the request and response bodies.
#[derive(Clone)]
pub struct ExchangeTap {
    tx: mpsc::Sender<(Stamp, Msg)>,
    clock: Clock,
    recorder: Recorder,
    stats: Arc<Stats>,
    /// Per direction: a chunk was refused and the next one queued must say so.
    gap: Arc<[AtomicBool; 2]>,
    /// The response head switched protocols, so later bytes are frames.
    websocket: Arc<AtomicBool>,
}

impl ExchangeTap {
    pub fn exchange_id(&self) -> u64 {
        self.recorder.exchange
    }
    /// Spawns the tap task; it writes `exchange_end` only once `start_edges` resolves.
    pub(super) fn spawn(
        recording: Recording,
        clock: Clock,
        parser: Box<dyn ExchangeParser>,
        start_edges: JoinHandle<()>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(TAP_QUEUE);
        let stats = Arc::new(Stats::new());
        let recorder = recording.recorder.clone();
        tokio::spawn(
            TapTask {
                rx,
                parser,
                recording,
                start_edges,
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
            gap: Default::default(),
            websocket: Default::default(),
        }
    }

    fn note(&self, note: Note) {
        self.recorder.write(self.clock.stamp(), Entry::Note(note), (0, 0), None);
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
            self.websocket.store(true, Ordering::Relaxed);
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
        self.offer(Msg::Input(TapInput::ResponseHead(ResponseHead {
            status: res.status().as_u16(),
            headers,
        })));
    }

    /// Hands in the bytes one side sent: counted first, then queued for the parser. Never waits.
    pub fn bytes(&self, side: Side, bytes: Bytes) {
        if side == Side::Response {
            self.stats.response_chunk_arrived(bytes.len());
        }
        let msg = if self.websocket.load(Ordering::Relaxed) {
            let from_client = side == Side::Request;
            let after_gap = self.gap[from_client as usize].swap(false, Ordering::Relaxed);
            Msg::WsBytes {
                from_client,
                bytes,
                after_gap,
            }
        } else {
            Msg::Input(match side {
                Side::Request => TapInput::RequestBody(bytes),
                Side::Response => TapInput::ResponseBody(bytes),
            })
        };
        self.offer(msg);
    }

    /// Says one side's body has ended.
    pub fn end(&self, side: Side) {
        self.offer(Msg::Input(match side {
            Side::Request => TapInput::RequestEnd,
            Side::Response => TapInput::ResponseEnd,
        }));
    }

    /// Adds how long the forwarding path held a chunk before handing it on.
    pub fn handed_on(&self, elapsed: Duration) {
        self.stats.chunk_handed_on(elapsed);
    }

    fn offer(&self, msg: Msg) {
        if let Err(refused) = self.tx.try_send((self.clock.stamp(), msg)) {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            if let (_, Msg::WsBytes { from_client, .. }) = refused.into_inner() {
                self.gap[from_client as usize].store(true, Ordering::Relaxed);
            }
        }
    }
}

/// What an exchange's records are written with: its recorder, its steps and the project they wait for.
pub(super) struct Recording {
    pub recorder: Recorder,
    pub steps: Steps,
    pub project: Project,
}

impl Recording {
    /// Writes a record once its project is settled and the earlier exchanges of its thread have written
    /// theirs, then the edges of its step, then lets the later exchanges of the thread go ahead.
    async fn write_record(&self, stamp: Stamp, mut record: Record) {
        let project = self.project.settled().await;
        let edges = self.steps.link(&mut record).await;
        self.recorder.write_record(stamp, record, project).await;
        for edge in edges {
            self.recorder.write_edge(stamp, edge).await;
        }
        self.steps.leave();
    }
}

struct TapTask {
    rx: mpsc::Receiver<(Stamp, Msg)>,
    parser: Box<dyn ExchangeParser>,
    recording: Recording,
    start_edges: JoinHandle<()>,
    clock: Clock,
    stats: Arc<Stats>,
}

impl TapTask {
    async fn run(self) {
        let Self {
            mut rx,
            mut parser,
            mut recording,
            start_edges,
            clock,
            stats,
        } = self;
        let mut decoders = [WsDecoder::default(), WsDecoder::default()];
        let mut assembler = Assembler::default();
        let dropped = || stats.dropped.load(Ordering::Relaxed) + recording.recorder.refused();
        while let Some((stamp, msg)) = rx.recv().await {
            let inputs = match msg {
                Msg::Input(input) => vec![input],
                Msg::WsBytes {
                    from_client,
                    bytes,
                    after_gap,
                } => {
                    let decoder = &mut decoders[from_client as usize];
                    if after_gap {
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
                let step = recording.steps.of_line(&event.event);
                let records = assembler.push(&event.event, dropped());
                recording
                    .recorder
                    .write(stamp, Entry::Event(event.event), (event.start, event.end), step);
                for record in records {
                    recording.write_record(stamp, record).await;
                }
            }
        }
        // A project task that panicked or was cancelled has nothing more to write
        let _ = start_edges.await;
        if let Some(record) = assembler.finish(dropped()) {
            recording.write_record(clock.stamp(), record).await;
        }
        let recorder = &recording.recorder;
        let end = Note::ExchangeEnd(stats.summary(recorder.output.sink.dropped()));
        recorder.write_reliable(clock.stamp(), Entry::Note(end)).await;
    }
}
