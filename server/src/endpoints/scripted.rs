//! Post-handshake fault endpoints built on [`Session`]. Each one has
//! already sent a correct `101` when it is called.

use std::{io, time::Duration};

use tokio::io::AsyncReadExt;

use crate::{
    frame::{
        self, End, Event, OP_BINARY, OP_CLOSE, OP_CONT, OP_PING, OP_PONG, OP_TEXT, Session, Stop,
        Trigger, close_payload,
    },
    http::{Query, hex_decode},
};

/// Echoes until the client closes, answering its close normally.
pub async fn serve_echo(mut s: Session) -> io::Result<()> {
    if let Stop::Closed(p) = s.echo_until(Trigger::never()).await? {
        s.reply_close(&p).await;
    }
    Ok(())
}

/// `/close?code=&reason=&after_ms=&after_msgs=&then=wait|fin|rst|hold`
/// (`/close-empty` passes `empty = true`).
pub async fn close(mut s: Session, q: &Query, empty: bool) -> io::Result<()> {
    let trigger = Trigger::after(q.num("after_msgs", 0), q.num("after_ms", 0));
    match s.echo_until(trigger).await? {
        Stop::Trigger => {
            let code = if empty {
                None
            } else {
                q.get("code").and_then(|c| c.parse().ok())
            };
            let payload = close_payload(code, q.str_or("reason", ""));
            s.send(OP_CLOSE, &payload).await?;
            s.finish(End::parse(q.get("then"), End::Wait)).await;
        }
        Stop::Closed(p) => s.reply_close(&p).await,
        Stop::Eof => {}
    }
    Ok(())
}

/// `/drop?after_ms=&after_msgs=&mode=fin|rst`: drops TCP without a close.
pub async fn drop_conn(mut s: Session, q: &Query) -> io::Result<()> {
    let trigger = Trigger::after(q.num("after_msgs", 0), q.num("after_ms", 0));
    match s.echo_until(trigger).await? {
        Stop::Trigger => s.finish(End::parse(q.get("mode"), End::Fin)).await,
        Stop::Closed(p) => s.reply_close(&p).await,
        Stop::Eof => {}
    }
    Ok(())
}

/// `/ignore-close`: echoes, but never answers a close frame.
pub async fn ignore_close(mut s: Session) -> io::Result<()> {
    if let Stop::Closed(_) = s.echo_until(Trigger::never()).await? {
        s.hold().await;
    }
    Ok(())
}

/// `/silent?read=1`: sends nothing, never pongs. `read=0` also stops
/// reading so the client's send window fills up.
pub async fn silent(mut s: Session, q: &Query) {
    if q.num("read", 1) != 0 {
        s.hold().await;
    } else {
        tokio::time::sleep(frame::HOLD_TIMEOUT).await;
    }
}

/// `/slow-read?bps=4096&pause_ms=0`: first stops reading for `pause_ms`,
/// then reads at most `bps` bytes per second (`0` = unlimited). Echoes
/// nothing; the byte count is visible in `/log`.
pub async fn slow_read(mut s: Session, q: &Query) {
    tokio::time::sleep(Duration::from_millis(q.num("pause_ms", 0))).await;
    let bps = q.num("bps", 4096) as usize;
    // Read in 10 ticks per second.
    let chunk = if bps == 0 {
        64 * 1024
    } else {
        (bps / 10).max(1)
    };
    let mut buf = vec![0u8; chunk];
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        if bps != 0 {
            tick.tick().await;
        }
        match s.conn.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

/// `/flood?count=10000&size=16&kind=text|binary&then=close|stay`
pub async fn flood(mut s: Session, q: &Query) -> io::Result<()> {
    let count = q.num("count", 10_000);
    let size = q.num("size", 16) as usize;
    let op = kind_op(q);
    let payload: Vec<u8> = (0..size).map(|i| b'a' + (i % 26) as u8).collect();
    let mut one = Vec::new();
    frame::encode(&mut one, true, 0, op, &payload);
    let per_batch = (64 * 1024 / one.len()).max(1) as u64;
    let mut sent = 0;
    while sent < count {
        let n = per_batch.min(count - sent);
        s.write(&one.repeat(n as usize)).await?;
        sent += n;
    }
    if q.get("then") == Some("stay") {
        return serve_echo(s).await;
    }
    s.send(OP_CLOSE, &close_payload(Some(1000), "done")).await?;
    s.finish(End::Wait).await;
    Ok(())
}

/// `/ping?payload=<hex>&interval_ms=0&count=1`: sends pings one at a time
/// and reports each answer as a text message `pong-ok` or `pong-mismatch`.
/// Then echoes.
pub async fn ping(mut s: Session, q: &Query) -> io::Result<()> {
    let payload = hex_decode(q.str_or("payload", "")).unwrap_or_default();
    let interval = q.num("interval_ms", 0);
    for i in 0..q.num("count", 1) {
        if i > 0 && interval > 0 {
            match s.echo_until(Trigger::after(0, interval)).await? {
                Stop::Trigger => {}
                Stop::Closed(p) => {
                    s.reply_close(&p).await;
                    return Ok(());
                }
                Stop::Eof => return Ok(()),
            }
        }
        s.send(OP_PING, &payload).await?;
        let answer = loop {
            match s.next().await {
                Event::Pong(p) => break p,
                Event::Message(op, data) => s.send(op, &data).await?,
                Event::Ping(p) => s.send(OP_PONG, &p).await?,
                Event::Close(p) => {
                    s.reply_close(&p).await;
                    return Ok(());
                }
                Event::Eof => return Ok(()),
            }
        };
        let verdict = if answer == payload {
            "pong-ok"
        } else {
            "pong-mismatch"
        };
        s.send(OP_TEXT, verdict.as_bytes()).await?;
    }
    serve_echo(s).await
}

/// `/pong?interval_ms=100`: sends unsolicited empty pongs, never answers
/// pings, echoes messages.
pub async fn pong(mut s: Session, q: &Query) -> io::Result<()> {
    let every = Duration::from_millis(q.num("interval_ms", 100).max(1));
    let mut tick = tokio::time::interval(every);
    loop {
        tokio::select! {
            _ = tick.tick() => s.send(OP_PONG, b"").await?,
            event = s.next_event() => match event? {
                Event::Message(op, data) => s.send(op, &data).await?,
                Event::Ping(_) | Event::Pong(_) => {}
                Event::Close(p) => {
                    s.reply_close(&p).await;
                    return Ok(());
                },
                Event::Eof => return Ok(()),
            },
        }
    }
}

/// `/stall?size=1000&sent=500`: a binary frame header for `size` bytes but
/// only `sent` payload bytes, then hold.
pub async fn stall(mut s: Session, q: &Query) -> io::Result<()> {
    let size = q.num("size", 1000);
    let sent = q.num("sent", 500).min(size) as usize;
    let mut out = vec![0x80 | OP_BINARY];
    frame::encode_len(&mut out, size, false);
    out.resize(out.len() + sent, 0x42);
    s.write(&out).await?;
    s.hold().await;
    Ok(())
}

/// `/fragment?size=1024&parts=4&kind=text|binary&ping_between=0|1&split_utf8=0|1`
/// Sends one message in `parts` fragments, then echoes.
///
/// Payloads: text is `a..z` repeated; with `split_utf8=1` it is `€`
/// (3 bytes) repeated, `size` rounded up to a multiple of 3, and every
/// boundary falls inside a code point. Binary is `i % 256`.
pub async fn fragment(mut s: Session, q: &Query) -> io::Result<()> {
    let op = kind_op(q);
    let size = q.num("size", 1024) as usize;
    let split_utf8 = op == OP_TEXT && q.flag("split_utf8");
    let payload = fragment_payload(op, size, split_utf8);
    let cuts = fragment_cuts(payload.len(), q.num("parts", 4) as usize, split_utf8);
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &end) in cuts.iter().enumerate() {
        let last = i + 1 == cuts.len();
        let frame_op = if i == 0 { op } else { OP_CONT };
        frame::encode(&mut out, last, 0, frame_op, &payload[start..end]);
        if !last && q.flag("ping_between") {
            frame::encode(&mut out, true, 0, OP_PING, b"between");
        }
        start = end;
    }
    s.write(&out).await?;
    serve_echo(s).await
}

fn fragment_payload(op: u8, size: usize, split_utf8: bool) -> Vec<u8> {
    match (op, split_utf8) {
        (OP_TEXT, true) => "€".repeat(size.div_ceil(3)).into_bytes(),
        (OP_TEXT, false) => (0..size).map(|i| b'a' + (i % 26) as u8).collect(),
        _ => (0..size).map(|i| i as u8).collect(),
    }
}

/// End offsets of each fragment. With `split_utf8`, cuts avoid multiples
/// of 3 so they land inside a `€`.
fn fragment_cuts(len: usize, parts: usize, split_utf8: bool) -> Vec<usize> {
    let parts = parts.clamp(1, len.max(1));
    let mut cuts = Vec::with_capacity(parts);
    let mut prev = 0;
    for k in 1..parts {
        let mut cut = len * k / parts;
        if split_utf8 && cut.is_multiple_of(3) {
            cut += 1;
        }
        if cut > prev && cut < len {
            cuts.push(cut);
            prev = cut;
        }
    }
    cuts.push(len);
    cuts
}

/// `kind=text|binary` (default text) as an opcode.
fn kind_op(q: &Query) -> u8 {
    if q.get("kind") == Some("binary") {
        OP_BINARY
    } else {
        OP_TEXT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_cuts_land_inside_code_points() {
        let cuts = fragment_cuts(30, 4, true);
        assert_eq!(*cuts.last().unwrap(), 30);
        assert!(cuts[..cuts.len() - 1].iter().all(|c| c % 3 != 0));
        assert_eq!(fragment_cuts(0, 4, false), vec![0]);
        assert_eq!(fragment_cuts(3, 10, false), vec![1, 2, 3]);
    }
}
