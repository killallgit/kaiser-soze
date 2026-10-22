use super::clock::{Clock, Stamp};
use super::recorder::Recorder;
use super::stats::Stats;
use crate::adapter::{ExchangeParser, TapEvent, TapInput};
use crate::assembler::Assembler;
use crate::ws::WsDecoder;
use bytes::Bytes;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
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
    pub stats: Arc<Stats>,
}

impl ExchangeTap {
    pub fn spawn(recorder: Recorder, clock: Clock, parser: Box<dyn ExchangeParser>) -> Self {
        let (tx, rx) = mpsc::channel(TAP_QUEUE);
        let stats = Arc::new(Stats::new());
        tokio::spawn(run_parser(rx, parser, recorder.clone(), clock, stats.clone()));
        Self {
            tx,
            clock,
            recorder,
            stats,
        }
    }

    pub fn project_looked_up(&self, project: Option<String>) {
        self.recorder.project_looked_up(project);
    }

    pub fn note(&self, kind: &str, payload: Value) {
        self.recorder.write(self.clock.stamp(), kind, payload, (0, 0));
    }

    /// Like `note`, but waits for room in the output queue. Only a tap-side task calls this.
    pub async fn note_reliable(&self, kind: &str, payload: Value) {
        self.recorder.write_reliable(self.clock.stamp(), kind, payload).await;
    }

    pub fn send(&self, input: TapInput) {
        self.offer(Msg::Input(input));
    }

    pub fn send_ws_bytes(&self, from_client: bool, bytes: Bytes) {
        self.offer(Msg::WsBytes { from_client, bytes });
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

async fn run_parser(
    mut rx: mpsc::Receiver<(Stamp, Msg)>,
    mut parser: Box<dyn ExchangeParser>,
    recorder: Recorder,
    clock: Clock,
    stats: Arc<Stats>,
) {
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
            let records = assembler.push(&event, dropped());
            let TapEvent {
                kind,
                payload,
                start,
                end,
            } = event;
            recorder.write(stamp, &kind, payload, (start, end));
            for record in records {
                recorder.write_record(stamp, record).await;
            }
        }
    }
    if let Some(record) = assembler.finish(dropped()) {
        recorder.write_record(clock.stamp(), record).await;
    }
    let mut summary = stats.summary();
    summary["sink_dropped"] = json!(recorder.sink.dropped());
    recorder.write_reliable(clock.stamp(), "exchange_end", summary).await;
}
