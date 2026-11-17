//! Benchmark endpoints, served through tungstenite: `/echo` and `/stream`.
//! Their wire behaviour must stay stable; the benchmarks depend on it.

use std::{
    fmt::Write as _,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

use crate::{
    BoxError,
    conn::Conn,
    frame::{End, Session},
    http::Query,
};

/// Sends every text/binary message back unchanged until the client closes.
pub async fn echo<S>(ws: &mut WebSocketStream<S>) -> Result<(), BoxError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(msg) = ws.next().await {
        match msg? {
            msg @ (Message::Text(_) | Message::Binary(_)) => ws.send(msg).await?,
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// `/echo` on an upgraded connection. When the connection is logged
/// (`id=`), the server also half-closes and waits for the client's FIN so
/// `/log` can report how TCP ended; otherwise it just drops the socket.
pub async fn echo_conn(ws: WebSocketStream<Conn>) -> Result<(), BoxError> {
    let mut ws = ws;
    echo(&mut ws).await?;
    if ws.get_ref().is_logged() {
        Session::new(ws.into_inner()).finish(End::Fin).await;
    }
    Ok(())
}

/// Sends a close frame with `code` and `reason`, then waits for the reply.
pub async fn close_with<S>(
    mut ws: WebSocketStream<S>,
    code: CloseCode,
    reason: String,
) -> Result<(), BoxError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    ws.close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
    .await?;
    while let Some(Ok(_)) = ws.next().await {}
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Json,
    Binary,
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

pub async fn stream_messages<S>(ws: WebSocketStream<S>, query: &Query) -> Result<(), BoxError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let get = |k: &str, d: u64| query.num(k, d);
    let format = match query.get("format") {
        Some("binary") => Format::Binary,
        _ => Format::Json,
    };
    let size = get("size", 1024) as usize;
    let rate = get("rate", 1000);
    let duration = get("duration", 5000);
    let count = query
        .get("count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(rate * duration / 1000);
    let unicode = get("unicode", 0) != 0;

    let (mut tx, mut rx) = ws.split();
    // Drain the client side (pongs, close) so the connection stays healthy.
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = rx.next().await {
            if msg.is_close() {
                break;
            }
        }
    });

    let payload = Payload::new(format, size, unicode);
    let start = Instant::now();
    let mut seq: u64 = 0;
    while seq < count {
        let due = if rate == 0 {
            count
        } else {
            (start.elapsed().as_micros() as u64 * rate / 1_000_000 + 1).min(count)
        };
        // In flood mode flush periodically so the client starts receiving early.
        let batch_end = if rate == 0 { (seq + 64).min(due) } else { due };
        while seq < batch_end {
            tx.feed(payload.message(seq, now_micros())).await?;
            seq += 1;
        }
        tx.flush().await?;
        if rate != 0 && seq < count {
            tokio::time::sleep(Duration::from_micros(500)).await;
        }
    }
    tx.send(Message::Close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "done".into(),
    })))
    .await?;
    let _ = tokio::time::timeout(Duration::from_secs(5), reader).await;
    Ok(())
}

struct Payload {
    format: Format,
    /// Pre-rendered JSON `items` array.
    json: String,
    /// Binary filler template.
    binary: Bytes,
}

impl Payload {
    fn new(format: Format, size: usize, unicode: bool) -> Self {
        match format {
            Format::Json => Self {
                format,
                json: json_items(size, unicode),
                binary: Bytes::new(),
            },
            Format::Binary => Self {
                format,
                json: String::new(),
                binary: (0..size.max(16)).map(|i| (i % 251) as u8).collect(),
            },
        }
    }

    fn message(&self, seq: u64, ts: u64) -> Message {
        match self.format {
            Format::Json => {
                let mut text = String::with_capacity(self.json.len() + 64);
                let _ = write!(text, "{{\"seq\":{seq},\"ts\":{ts},\"items\":");
                text.push_str(&self.json);
                text.push('}');
                Message::text(text)
            }
            Format::Binary => {
                let mut buf = BytesMut::from(&self.binary[..]);
                buf[0..8].copy_from_slice(&seq.to_le_bytes());
                buf[8..16].copy_from_slice(&ts.to_le_bytes());
                Message::Binary(buf.freeze())
            }
        }
    }
}

/// Renders a realistic JSON array of objects so that the whole message is
/// roughly `size` bytes.
fn json_items(size: usize, unicode: bool) -> String {
    const ENVELOPE: usize = 48; // {"seq":…,"ts":…,"items":…}
    let mut out = String::from("[");
    let mut i = 0u64;
    while out.len() + ENVELOPE < size {
        if i > 0 {
            out.push(',');
        }
        let name = if unicode {
            format!("Товар №{i} — ünïcødé ✓")
        } else {
            format!("item-{i}")
        };
        out.push_str(&format!(
            concat!(
                r#"{{"id":{id},"name":"{name}","price":{price:.2},"qty":{qty},"#,
                r#""active":{active},"tags":["alpha","beta","gamma"],"#,
                r#""meta":{{"score":{score:.4},"rank":{rank},"note":null}}}}"#
            ),
            id = 100_000 + i,
            name = name,
            price = (i as f64) * 1.37 + 0.99,
            qty = i % 17,
            active = i.is_multiple_of(2),
            score = (i as f64).sqrt() / 3.0,
            rank = i % 100,
        ));
        i += 1;
    }
    out.push(']');
    out
}
