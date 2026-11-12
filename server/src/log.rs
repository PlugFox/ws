//! Records what clients sent, for `/log?id=` and `/proxy-log`.
//!
//! A [`ConnLog`] is fed every byte read from a connection after its HTTP
//! head. A passive frame parser turns those bytes into frame summaries
//! (`op`, `fin`, `len`, `masked`, `rsv`) and the close code and reason,
//! without buffering payloads. Entries live for the whole process.

use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{Arc, Mutex},
};

use crate::http::json_str;

/// Frames kept per connection; later ones are only counted.
const MAX_FRAMES: usize = 100_000;

/// Close payload bytes kept (a valid close payload is at most 125).
const MAX_CLOSE: usize = 1024;

/// How the client's side of the TCP connection ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TcpEnd {
    Open,
    Fin,
    Rst,
}

#[derive(Clone, Copy)]
struct FrameInfo {
    op: u8,
    fin: bool,
    len: u64,
    masked: bool,
    rsv: u8,
}

/// The first close frame the client sent.
struct CloseInfo {
    /// Raw (unmasked) payload.
    payload: Vec<u8>,
}

/// Per-connection record.
#[derive(Default)]
struct State {
    frames: Vec<FrameInfo>,
    frames_dropped: u64,
    close: Option<CloseInfo>,
    bytes: u64,
    tcp: Option<TcpEnd>,
    done: bool,
    parser: Parser,
}

/// Shared handle to one connection's record.
#[derive(Default)]
pub struct ConnLog(Mutex<State>);

impl ConnLog {
    /// Feeds bytes the client sent.
    pub fn feed(&self, data: &[u8]) {
        let mut st = self.0.lock().unwrap();
        st.bytes += data.len() as u64;
        let State {
            parser,
            frames,
            frames_dropped,
            close,
            ..
        } = &mut *st;
        parser.feed(data, &mut |event| match event {
            Event::Frame(f) if frames.len() < MAX_FRAMES => frames.push(f),
            Event::Frame(_) => *frames_dropped += 1,
            Event::Close(payload) => {
                close.get_or_insert(CloseInfo { payload });
            }
        });
    }

    /// Records how the read side ended (first call wins).
    pub fn tcp_end(&self, end: TcpEnd) {
        let mut st = self.0.lock().unwrap();
        if st.tcp.is_none() || st.tcp == Some(TcpEnd::Open) {
            st.tcp = Some(end);
        }
    }

    /// Marks the server side as finished with the connection.
    pub fn finish(&self) {
        self.0.lock().unwrap().done = true;
    }

    /// Renders the record as JSON.
    pub fn to_json(&self, id: &str) -> String {
        let st = self.0.lock().unwrap();
        let mut out = String::from("{\"id\":");
        json_str(&mut out, id);
        out.push_str(",\"frames\":[");
        for (i, f) in st.frames.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"op\":{},\"fin\":{},\"len\":{},\"masked\":{},\"rsv\":{}}}",
                f.op, f.fin, f.len, f.masked, f.rsv
            );
        }
        let _ = write!(out, "],\"frames_dropped\":{},\"close\":", st.frames_dropped);
        match &st.close {
            None => out.push_str("null"),
            Some(c) => close_json(&mut out, &c.payload),
        }
        let tcp = match st.tcp.unwrap_or(TcpEnd::Open) {
            TcpEnd::Open => "open",
            TcpEnd::Fin => "fin",
            TcpEnd::Rst => "rst",
        };
        let _ = write!(
            out,
            ",\"bytes\":{},\"tcp\":\"{tcp}\",\"done\":{}}}",
            st.bytes, st.done
        );
        out
    }
}

/// `{"code":N|null,"reason":"...","len":N}` for a close payload.
fn close_json(out: &mut String, payload: &[u8]) {
    let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
    let reason = payload.get(2..).unwrap_or_default();
    match code {
        Some(c) => {
            let _ = write!(out, "{{\"code\":{c},\"reason\":");
        }
        None => out.push_str("{\"code\":null,\"reason\":"),
    }
    json_str(out, &String::from_utf8_lossy(reason));
    let _ = write!(
        out,
        ",\"reason_utf8\":{},\"len\":{}}}",
        std::str::from_utf8(reason).is_ok(),
        payload.len()
    );
}

enum Event {
    Frame(FrameInfo),
    Close(Vec<u8>),
}

/// Incremental frame parser that never buffers more than a header and a
/// close payload.
#[derive(Default)]
struct Parser {
    header: Vec<u8>,
    /// Payload bytes of the current frame still to skip.
    remaining: u64,
    /// Mask of the current frame and offset into its payload.
    mask: Option<[u8; 4]>,
    offset: u64,
    /// Collected payload when the current frame is a close frame.
    close: Option<Vec<u8>>,
}

impl Parser {
    fn feed(&mut self, mut data: &[u8], emit: &mut dyn FnMut(Event)) {
        while !data.is_empty() {
            if self.remaining > 0 {
                let n = (self.remaining).min(data.len() as u64) as usize;
                self.collect_close(&data[..n]);
                self.remaining -= n as u64;
                data = &data[n..];
                if self.remaining == 0 {
                    self.end_payload(emit);
                }
                continue;
            }
            self.header.push(data[0]);
            data = &data[1..];
            if let Some(frame) = self.try_header() {
                self.start_frame(frame, emit);
            }
        }
    }

    /// Parses the buffered header once it is complete.
    fn try_header(&mut self) -> Option<FrameInfo> {
        let h = &self.header;
        if h.len() < 2 {
            return None;
        }
        let masked = h[1] & 0x80 != 0;
        let ext = match h[1] & 0x7F {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let need = 2 + ext + if masked { 4 } else { 0 };
        if h.len() < need {
            return None;
        }
        let len = match ext {
            0 => (h[1] & 0x7F) as u64,
            n => h[2..2 + n].iter().fold(0u64, |acc, b| acc << 8 | *b as u64),
        };
        self.mask = masked.then(|| h[need - 4..need].try_into().unwrap());
        Some(FrameInfo {
            op: h[0] & 0x0F,
            fin: h[0] & 0x80 != 0,
            len,
            masked,
            rsv: (h[0] >> 4) & 0x07,
        })
    }

    fn start_frame(&mut self, frame: FrameInfo, emit: &mut dyn FnMut(Event)) {
        self.header.clear();
        self.offset = 0;
        self.remaining = frame.len;
        self.close = (frame.op == 8).then(Vec::new);
        emit(Event::Frame(frame));
        if frame.len == 0 {
            self.end_payload(emit);
        }
    }

    fn collect_close(&mut self, chunk: &[u8]) {
        if let Some(buf) = &mut self.close {
            for &b in chunk {
                let m = self.mask.map_or(0, |m| m[(self.offset % 4) as usize]);
                if buf.len() < MAX_CLOSE {
                    buf.push(b ^ m);
                }
                self.offset += 1;
            }
        }
    }

    fn end_payload(&mut self, emit: &mut dyn FnMut(Event)) {
        if let Some(payload) = self.close.take() {
            emit(Event::Close(payload));
        }
    }
}

/// Process-wide (per server instance) logs.
#[derive(Default)]
pub struct Logs {
    conns: Mutex<HashMap<String, Arc<ConnLog>>>,
    tunnels: Mutex<Vec<String>>,
}

impl Logs {
    /// Starts a fresh record for `id`, replacing an older one.
    pub fn open(&self, id: &str) -> Arc<ConnLog> {
        let log = Arc::new(ConnLog::default());
        self.conns
            .lock()
            .unwrap()
            .insert(id.to_owned(), log.clone());
        log
    }

    /// JSON for `/log?id=`, or `None` when unknown.
    pub fn conn_json(&self, id: &str) -> Option<String> {
        let log = self.conns.lock().unwrap().get(id).cloned()?;
        Some(log.to_json(id))
    }

    /// Records a CONNECT target.
    pub fn tunnel(&self, target: &str) {
        self.tunnels.lock().unwrap().push(target.to_owned());
    }

    /// JSON for `/proxy-log`.
    pub fn tunnels_json(&self) -> String {
        let mut out = String::from("{\"tunnels\":[");
        for (i, t) in self.tunnels.lock().unwrap().iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            json_str(&mut out, t);
        }
        out.push_str("]}");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_masked_frames_split_anywhere() {
        // Masked text "hi", then masked close 1000 "ok".
        let mask = [1u8, 2, 3, 4];
        let mut bytes = vec![0x81, 0x82];
        bytes.extend(mask);
        bytes.extend(b"hi".iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
        bytes.extend([0x88, 0x84]);
        bytes.extend(mask);
        bytes.extend(
            [0x03, 0xE8, b'o', b'k']
                .iter()
                .zip(mask.iter().cycle())
                .map(|(b, m)| b ^ m),
        );
        let log = ConnLog::default();
        for b in &bytes {
            log.feed(std::slice::from_ref(b));
        }
        let json = log.to_json("x");
        assert!(
            json.contains("{\"op\":1,\"fin\":true,\"len\":2,\"masked\":true,\"rsv\":0}"),
            "{json}"
        );
        assert!(json.contains("\"code\":1000,\"reason\":\"ok\""), "{json}");
        assert!(
            json.contains(&format!("\"bytes\":{}", bytes.len())),
            "{json}"
        );
    }
}
