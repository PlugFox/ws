//! Hand-rolled WebSocket framing for the scripted endpoints, which need to
//! do things tungstenite would not (ignore a close, never pong, stall in the
//! middle of a frame, end the TCP connection in a chosen way).

use std::{
    io,
    time::{Duration, Instant},
};

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::conn::Conn;

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

/// Largest client frame the scripted endpoints accept.
const MAX_FRAME: u64 = 256 << 20;

/// How long `wait`/`fin` keep reading for the client's FIN.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound for `hold`, so forgotten connections eventually go away.
pub const HOLD_TIMEOUT: Duration = Duration::from_secs(600);

/// Appends an unmasked server frame to `out`.
pub fn encode(out: &mut Vec<u8>, fin: bool, rsv: u8, op: u8, payload: &[u8]) {
    out.push((fin as u8) << 7 | (rsv & 7) << 4 | (op & 0x0F));
    encode_len(out, payload.len() as u64, false);
    out.extend_from_slice(payload);
}

/// Appends a payload length (second header byte plus extension) using the
/// minimal encoding.
pub fn encode_len(out: &mut Vec<u8>, len: u64, masked: bool) {
    let m = (masked as u8) << 7;
    if len < 126 {
        out.push(m | len as u8);
    } else if len <= u16::MAX as u64 {
        out.push(m | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(m | 127);
        out.extend_from_slice(&len.to_be_bytes());
    }
}

/// A close payload: optional code plus reason.
pub fn close_payload(code: Option<u16>, reason: &str) -> Vec<u8> {
    match code {
        None => Vec::new(),
        Some(c) => {
            let mut p = c.to_be_bytes().to_vec();
            p.extend_from_slice(reason.as_bytes());
            p
        }
    }
}

/// One frame received from the client (payload unmasked).
pub struct Frame {
    pub fin: bool,
    pub op: u8,
    pub payload: Vec<u8>,
}

/// What the client did, at message level.
pub enum Event {
    /// A complete text or binary message.
    Message(u8, Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// A close frame, with its raw payload.
    Close(Vec<u8>),
    /// TCP read side ended (EOF or error).
    Eof,
}

/// How a scripted connection ends (`then=` / `end=` / `mode=`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End {
    /// shutdown(Write), then close.
    Fin,
    /// SO_LINGER 0, then close.
    Rst,
    /// Keep TCP open, read and discard until the client goes.
    Hold,
    /// Wait for the client's close frame, then FIN, then wait for its FIN.
    Wait,
}

impl End {
    pub fn parse(s: Option<&str>, default: End) -> End {
        match s {
            Some("fin") => End::Fin,
            Some("rst") => End::Rst,
            Some("hold") => End::Hold,
            Some("wait") => End::Wait,
            _ => default,
        }
    }
}

/// A connection after the 101, read frame by frame.
pub struct Session {
    pub conn: Conn,
    buf: BytesMut,
    /// Opcode and data of a fragmented message in progress.
    partial: Option<(u8, Vec<u8>)>,
}

impl Session {
    pub fn new(conn: Conn) -> Self {
        Session {
            conn,
            buf: BytesMut::with_capacity(8192),
            partial: None,
        }
    }

    /// Reads one frame; `None` on EOF. Cancel-safe: partial input stays in
    /// the buffer.
    pub async fn read_frame(&mut self) -> io::Result<Option<Frame>> {
        loop {
            if let Some(frame) = self.parse_frame()? {
                return Ok(Some(frame));
            }
            if self.conn.read_buf(&mut self.buf).await? == 0 {
                return Ok(None);
            }
        }
    }

    /// Takes one complete frame off the buffer, if there is one.
    fn parse_frame(&mut self) -> io::Result<Option<Frame>> {
        let b = &self.buf[..];
        if b.len() < 2 {
            return Ok(None);
        }
        let masked = b[1] & 0x80 != 0;
        let ext = match b[1] & 0x7F {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let head = 2 + ext + if masked { 4 } else { 0 };
        if b.len() < head {
            return Ok(None);
        }
        let len = match ext {
            0 => (b[1] & 0x7F) as u64,
            n => b[2..2 + n].iter().fold(0u64, |acc, x| acc << 8 | *x as u64),
        };
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        if (b.len() as u64) < head as u64 + len {
            self.buf.reserve(head + len as usize - b.len());
            return Ok(None);
        }
        let (fin, op) = (b[0] & 0x80 != 0, b[0] & 0x0F);
        let mask: Option<[u8; 4]> = masked.then(|| b[head - 4..head].try_into().unwrap());
        self.buf.advance(head);
        let mut payload = self.buf.split_to(len as usize).to_vec();
        if let Some(m) = mask {
            payload
                .iter_mut()
                .enumerate()
                .for_each(|(i, x)| *x ^= m[i % 4]);
        }
        Ok(Some(Frame { fin, op, payload }))
    }

    /// Reads until a message-level event. Fragments are reassembled;
    /// protocol violations by the client are tolerated. Cancel-safe.
    pub async fn next_event(&mut self) -> io::Result<Event> {
        loop {
            let Some(f) = self.read_frame().await? else {
                return Ok(Event::Eof);
            };
            match f.op {
                OP_PING => return Ok(Event::Ping(f.payload)),
                OP_PONG => return Ok(Event::Pong(f.payload)),
                OP_CLOSE => return Ok(Event::Close(f.payload)),
                OP_TEXT | OP_BINARY if f.fin => return Ok(Event::Message(f.op, f.payload)),
                OP_TEXT | OP_BINARY => self.partial = Some((f.op, f.payload)),
                OP_CONT => {
                    if let Some((_, data)) = &mut self.partial {
                        data.extend_from_slice(&f.payload);
                    }
                    if f.fin
                        && let Some((op, data)) = self.partial.take()
                    {
                        return Ok(Event::Message(op, data));
                    }
                }
                _ => {}
            }
        }
    }

    /// Like [`next_event`](Self::next_event) but treats a read error as EOF.
    pub async fn next(&mut self) -> Event {
        self.next_event().await.unwrap_or(Event::Eof)
    }

    /// Sends one unfragmented frame.
    pub async fn send(&mut self, op: u8, payload: &[u8]) -> io::Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 10);
        encode(&mut out, true, 0, op, payload);
        self.write(&out).await
    }

    /// Writes raw bytes and flushes.
    pub async fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.conn.write_all(bytes).await?;
        self.conn.flush().await
    }

    /// Echoes messages and answers pings until `trigger` fires, the client
    /// sends a close frame or the connection ends.
    pub async fn echo_until(&mut self, trigger: Trigger) -> io::Result<Stop> {
        let mut echoed = 0u64;
        let deadline = tokio::time::Instant::from_std(trigger.deadline);
        loop {
            let armed = trigger.after_msgs.is_some_and(|n| echoed >= n);
            let event = tokio::select! {
                e = self.next_event() => e?,
                _ = tokio::time::sleep_until(deadline), if armed => return Ok(Stop::Trigger),
            };
            match event {
                Event::Message(op, data) => {
                    self.send(op, &data).await?;
                    echoed += 1;
                }
                Event::Ping(data) => self.send(OP_PONG, &data).await?,
                Event::Pong(_) => {}
                Event::Close(payload) => return Ok(Stop::Closed(payload)),
                Event::Eof => return Ok(Stop::Eof),
            }
        }
    }

    /// Answers a client-initiated close by echoing its status code, then
    /// ends like `wait` after our close.
    pub async fn reply_close(mut self, payload: &[u8]) {
        let reply = payload.get(..2).unwrap_or_default().to_vec();
        if self.send(OP_CLOSE, &reply).await.is_ok() {
            self.fin_and_drain().await;
        }
    }

    /// Ends the connection as `end` says. For `Wait` the server has already
    /// sent its close frame.
    pub async fn finish(mut self, end: End) {
        match end {
            End::Fin => self.fin_and_drain().await,
            End::Rst => {
                // Give the client a moment to read what was just written;
                // an RST can make the peer discard unread data.
                tokio::time::sleep(Duration::from_millis(20)).await;
                self.conn.reset()
            }
            End::Hold => self.hold().await,
            End::Wait => {
                let wait = async {
                    loop {
                        match self.next().await {
                            Event::Close(_) | Event::Eof => break,
                            _ => {}
                        }
                    }
                };
                let _ = tokio::time::timeout(HOLD_TIMEOUT, wait).await;
                self.fin_and_drain().await;
            }
        }
    }

    /// Reads and discards until the client goes away.
    pub async fn hold(&mut self) {
        let _ = tokio::time::timeout(HOLD_TIMEOUT, self.drain()).await;
    }

    /// shutdown(Write), then reads until the client's FIN (bounded), so the
    /// close is seen as FIN rather than an RST caused by unread data.
    async fn fin_and_drain(&mut self) {
        let _ = self.conn.shutdown().await;
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, self.drain()).await;
    }

    async fn drain(&mut self) {
        self.buf.clear();
        let mut scratch = vec![0u8; 16 * 1024];
        while let Ok(n) = self.conn.read(&mut scratch).await {
            if n == 0 {
                break;
            }
        }
    }
}

/// When [`Session::echo_until`] stops echoing.
#[derive(Clone, Copy)]
pub struct Trigger {
    /// Fire once this many messages were echoed (`None`: never fire).
    pub after_msgs: Option<u64>,
    /// ... and this instant has passed.
    pub deadline: Instant,
}

impl Trigger {
    pub fn never() -> Self {
        Trigger {
            after_msgs: None,
            deadline: Instant::now(),
        }
    }

    pub fn after(msgs: u64, ms: u64) -> Self {
        Trigger {
            after_msgs: Some(msgs),
            deadline: Instant::now() + Duration::from_millis(ms),
        }
    }
}

/// Why [`Session::echo_until`] returned.
pub enum Stop {
    Trigger,
    Closed(Vec<u8>),
    Eof,
}
