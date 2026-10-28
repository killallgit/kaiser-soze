use flate2::{Decompress, FlushDecompress, Status};

const MAX_FRAME: u64 = 64 * 1024 * 1024;
const DEFLATE_TAIL: [u8; 4] = [0, 0, 0xff, 0xff];

pub(super) struct WsText {
    pub(super) text: String,
    pub(super) start: u64,
    pub(super) end: u64,
}

pub(super) struct WsDecoder {
    buf: Vec<u8>,
    consumed: u64,
    message: Vec<u8>,
    message_start: u64,
    message_is_text: bool,
    message_compressed: bool,
    inflater: Decompress,
    broken: bool,
}

impl Default for WsDecoder {
    fn default() -> Self {
        Self {
            buf: Vec::new(),
            consumed: 0,
            message: Vec::new(),
            message_start: 0,
            message_is_text: false,
            message_compressed: false,
            inflater: Decompress::new(false),
            broken: false,
        }
    }
}

struct Frame {
    fin: bool,
    compressed: bool,
    opcode: u8,
    payload: Vec<u8>,
    len: usize,
}

impl WsDecoder {
    pub(super) fn push(&mut self, chunk: &[u8]) -> Vec<WsText> {
        if self.broken {
            return Vec::new();
        }
        self.buf.extend_from_slice(chunk);
        let mut texts = Vec::new();
        while let Some(frame) = self.next_frame() {
            let frame_start = self.consumed;
            self.buf.drain(..frame.len);
            self.consumed += frame.len as u64;
            texts.extend(self.accept(frame, frame_start));
        }
        texts
    }

    fn next_frame(&mut self) -> Option<Frame> {
        let buf = &self.buf;
        if buf.len() < 2 {
            return None;
        }
        let masked = buf[1] & 0x80 != 0;
        let (payload_len, mut at) = match buf[1] & 0x7f {
            126 if buf.len() >= 4 => (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4),
            127 if buf.len() >= 10 => (u64::from_be_bytes(buf[2..10].try_into().ok()?), 10),
            126 | 127 => return None,
            short => (short as u64, 2),
        };
        if payload_len > MAX_FRAME {
            self.broken = true;
            return None;
        }
        let key = if masked {
            let key = buf.get(at..at + 4)?.try_into().ok()?;
            at += 4;
            Some::<[u8; 4]>(key)
        } else {
            None
        };
        let end = at + payload_len as usize;
        let mut payload = buf.get(at..end)?.to_vec();
        if let Some(key) = key {
            payload.iter_mut().enumerate().for_each(|(i, byte)| *byte ^= key[i % 4]);
        }
        Some(Frame {
            fin: buf[0] & 0x80 != 0,
            compressed: buf[0] & 0x40 != 0,
            opcode: buf[0] & 0x0f,
            payload,
            len: end,
        })
    }

    fn accept(&mut self, frame: Frame, frame_start: u64) -> Option<WsText> {
        match frame.opcode {
            1 | 2 => {
                self.message.clear();
                self.message_start = frame_start;
                self.message_is_text = frame.opcode == 1;
                self.message_compressed = frame.compressed;
            }
            0 => {}
            _ => return None,
        }
        self.message.extend_from_slice(&frame.payload);
        if !frame.fin {
            return None;
        }
        let message = std::mem::take(&mut self.message);
        let message = if self.message_compressed {
            self.inflate(message)?
        } else {
            message
        };
        let text = String::from_utf8(message).ok().filter(|_| self.message_is_text)?;
        Some(WsText {
            text,
            start: self.message_start,
            end: self.consumed,
        })
    }

    fn inflate(&mut self, mut message: Vec<u8>) -> Option<Vec<u8>> {
        message.extend(DEFLATE_TAIL);
        let before = self.inflater.total_in();
        let mut out = Vec::with_capacity(message.len() * 4);
        loop {
            let used = (self.inflater.total_in() - before) as usize;
            let produced = out.len();
            out.reserve(8192);
            let step = self
                .inflater
                .decompress_vec(&message[used..], &mut out, FlushDecompress::Sync);
            let now_used = (self.inflater.total_in() - before) as usize;
            let stalled = now_used < message.len() && now_used == used && out.len() == produced;
            if matches!(step, Err(_) | Ok(Status::StreamEnd)) || stalled || out.len() as u64 > MAX_FRAME {
                self.broken = true;
                return None;
            }
            if now_used >= message.len() && out.len() < out.capacity() {
                return Some(out);
            }
        }
    }

    pub(super) fn mark_broken(&mut self) {
        self.broken = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};
    use std::time::Duration;

    fn frame(opcode: u8, fin: bool, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        let mask_bit = if mask.is_some() { 0x80 } else { 0 };
        match payload.len() {
            0..=125 => out.push(mask_bit | payload.len() as u8),
            _ => {
                out.push(mask_bit | 126);
                out.extend((payload.len() as u16).to_be_bytes());
            }
        }
        match mask {
            Some(key) => {
                out.extend(key);
                out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
            }
            None => out.extend(payload),
        }
        out
    }

    #[test]
    fn decodes_masked_and_unmasked_text_frames_split_anywhere() {
        // It yields each text message once, however the bytes are chunked
        let mut wire = frame(1, true, b"hello", Some([1, 2, 3, 4]));
        wire.extend(frame(1, true, &[b'x'; 300], None));
        let mut decoder = WsDecoder::default();

        let texts: Vec<WsText> = wire.chunks(7).flat_map(|c| decoder.push(c)).collect();

        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0].text, "hello");
        assert_eq!((texts[0].start, texts[0].end), (0, 11));
        assert_eq!(texts[1].text.len(), 300);
    }

    fn deflate_message(compressor: &mut Compress, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 64);
        compressor
            .compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync)
            .unwrap();
        out.truncate(out.len() - 4);
        let mut frame = vec![0xc1, out.len() as u8];
        frame.extend(out);
        frame
    }

    #[test]
    fn inflates_permessage_deflate_messages_sharing_one_context() {
        // It decodes compressed messages whose second one back-references the first
        let mut compressor = Compress::new(Compression::default(), false);
        let mut wire = deflate_message(&mut compressor, "the same words, again and again");
        wire.extend(deflate_message(&mut compressor, "the same words, again and again"));
        let mut decoder = WsDecoder::default();

        let texts = decoder.push(&wire);

        assert_eq!(texts.len(), 2);
        assert_eq!(texts[1].text, "the same words, again and again");
    }

    #[test]
    fn inflates_messages_whose_size_matches_a_buffer_boundary() {
        // It keeps a message whose inflated size is exactly a power of two
        for size in [8192, 16384, 32768] {
            let text = "a".repeat(size);
            let mut compressor = Compress::new(Compression::default(), false);
            let mut out = Vec::with_capacity(size);
            compressor
                .compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync)
                .unwrap();
            out.truncate(out.len() - 4);
            let mut wire = vec![0xc1, 126];
            wire.extend((out.len() as u16).to_be_bytes());
            wire.extend(out);

            let texts = WsDecoder::default().push(&wire);

            assert_eq!(texts.len(), 1, "size {size}");
            assert_eq!(texts[0].text.len(), size);
        }
    }

    #[test]
    fn a_final_deflate_block_stops_decoding_without_hanging() {
        // It gives up on a message that ends the deflate stream, and returns promptly
        let mut compressor = Compress::new(Compression::default(), false);
        let mut out = Vec::with_capacity(256);
        compressor
            .compress_vec(b"final block", &mut out, FlushCompress::Finish)
            .unwrap();
        let mut wire = vec![0xc1, out.len() as u8];
        wire.extend(out);
        let (done, finished) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let mut decoder = WsDecoder::default();
            done.send(decoder.push(&wire).len()).ok();
        });

        assert_eq!(finished.recv_timeout(Duration::from_secs(3)), Ok(0));
    }

    #[test]
    fn joins_fragments_and_skips_control_frames() {
        // It reassembles a fragmented message and ignores a ping between fragments
        let mut wire = frame(1, false, b"ab", None);
        wire.extend(frame(9, true, b"p", None));
        wire.extend(frame(0, true, b"cd", None));
        let mut decoder = WsDecoder::default();

        let texts = decoder.push(&wire);

        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].text, "abcd");
    }
}
