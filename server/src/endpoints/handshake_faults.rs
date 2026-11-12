//! Handshake-level endpoints: plain HTTP answers, broken or odd `101`s.

use std::{fmt::Write as _, io};

use tokio::io::AsyncWriteExt;

use crate::{
    BoxError,
    conn::Conn,
    endpoints::bench,
    frame::Session,
    handshake::{self, Head101},
    http::{self, Request, json_str},
};

/// `/status?code=403&body=text&header=Name:Value...`
pub async fn status(mut conn: Conn, req: &Request) -> io::Result<()> {
    let code = req.query.num("code", 200) as u16;
    let headers: Vec<(&str, &str)> = req
        .query
        .all("header")
        .filter_map(|h| h.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let body = req.query.str_or("body", "");
    http::respond(&mut conn, code, &headers, body.as_bytes()).await
}

/// `/redirect?code=302&to=/echo`
pub async fn redirect(mut conn: Conn, req: &Request) -> io::Result<()> {
    let code = req.query.num("code", 302) as u16;
    let to = req.query.str_or("to", "/echo");
    http::respond(&mut conn, code, &[("Location", to)], b"").await
}

/// `/redirect-loop?n=3`: redirects to itself with `n-1`, then to `/echo`.
/// Other query parameters (e.g. `id`) are carried along.
pub async fn redirect_loop(mut conn: Conn, req: &Request) -> io::Result<()> {
    let code = req.query.num("code", 302) as u16;
    let n = req.query.num("n", 3);
    let mut location = if n == 0 {
        String::from("/echo?")
    } else {
        format!("/redirect-loop?n={}", n - 1)
    };
    for (k, v) in req.query.0.iter().filter(|(k, _)| k != "n") {
        if !location.ends_with('?') {
            location.push('&');
        }
        let _ = write!(
            location,
            "{}={}",
            http::percent_encode(k),
            http::percent_encode(v)
        );
    }
    let location = location.trim_end_matches('?');
    http::respond(&mut conn, code, &[("Location", location)], b"").await
}

/// `/hang`: never answers; holds the connection until the client leaves.
pub async fn hang(conn: Conn) {
    Session::new(conn).hold().await;
}

/// `/eof-handshake?partial=0|1`: closes instead of answering
/// (`partial=1` first writes the start of a status line).
pub async fn eof_handshake(mut conn: Conn, req: &Request) -> io::Result<()> {
    if req.query.flag("partial") {
        conn.write_all(b"HTTP/1.1 101 Swi").await?;
    }
    conn.shutdown().await
}

/// `/junk`: answers with a non-HTTP greeting and closes.
pub async fn junk(mut conn: Conn) -> io::Result<()> {
    conn.write_all(b"220 smtp.example ESMTP\r\n").await?;
    conn.shutdown().await
}

/// `/big-head?kb=64`: a `101` with a filler header of `kb` KiB, then echo.
pub async fn big_head(mut conn: Conn, req: &Request) -> Result<(), BoxError> {
    let size = req.query.num("kb", 64) as usize * 1024;
    let mut head = Head101::new(req);
    head.push("X-Filler", "a".repeat(size));
    handshake::write_101(&mut conn, &head).await?;
    bench::echo_conn(handshake::websocket(conn).await).await
}

/// `/early-hints?count=1`: `count` informational `103 Early Hints`
/// responses, then the `101`, then echo.
pub async fn early_hints(mut conn: Conn, req: &Request) -> Result<(), BoxError> {
    for _ in 0..req.query.num("count", 1) {
        conn.write_all(b"HTTP/1.1 103 Early Hints\r\nLink: </style.css>; rel=preload\r\n\r\n")
            .await?;
    }
    handshake::write_101(&mut conn, &Head101::new(req)).await?;
    bench::echo_conn(handshake::websocket(conn).await).await
}

/// `/bad-101?omit=...&accept=wrong|dup|nonb64&upgrade=&connection=&protocol=&extensions=`
/// A crafted `101`; unset parts are correct. Continues as `/echo`.
pub async fn bad_101(mut conn: Conn, req: &Request) -> Result<(), BoxError> {
    handshake::write_101(&mut conn, &bad_head(req)).await?;
    bench::echo_conn(handshake::websocket(conn).await).await
}

fn bad_head(req: &Request) -> Head101 {
    let q = &req.query;
    let mut head = Head101::new(req);
    let accept = handshake::accept_key(req);
    match q.get("accept") {
        Some("wrong") => head.set("Sec-WebSocket-Accept", Some(tungstenite_key("wrong"))),
        Some("dup") => head.push("Sec-WebSocket-Accept", accept),
        Some("nonb64") => head.set("Sec-WebSocket-Accept", Some("!!not*base64!!".into())),
        _ => {}
    }
    if let Some(v) = q.get("upgrade") {
        head.set("Upgrade", Some(v.into()));
    }
    if let Some(v) = q.get("connection") {
        head.set("Connection", Some(v.into()));
    }
    if let Some(v) = q.get("extensions") {
        head.push("Sec-WebSocket-Extensions", v.into());
    }
    for part in q.all("omit").flat_map(|v| v.split(',')) {
        let name = match part.trim() {
            "accept" => "Sec-WebSocket-Accept",
            "upgrade" => "Upgrade",
            "connection" => "Connection",
            "protocol" => "Sec-WebSocket-Protocol",
            _ => continue,
        };
        head.set(name, None);
    }
    head
}

/// A well-formed accept value for some other key.
fn tungstenite_key(seed: &str) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(seed.as_bytes())
}

/// `/headers`: a `101`, then one text message describing the request, then
/// echo.
pub async fn headers(mut conn: Conn, req: &Request) -> Result<(), BoxError> {
    let json = request_json(req, conn.peer);
    handshake::upgrade(&mut conn, req).await?;
    let mut ws = handshake::websocket(conn).await;
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::text(json))
        .await?;
    bench::echo_conn(ws).await
}

/// `{method, target, version, headers:[[k,v],...], peer}`.
fn request_json(req: &Request, peer: Option<std::net::SocketAddr>) -> String {
    let mut out = String::from("{\"method\":");
    json_str(&mut out, &req.method);
    out.push_str(",\"target\":");
    json_str(&mut out, &req.target);
    out.push_str(",\"version\":");
    json_str(&mut out, &req.version);
    out.push_str(",\"headers\":[");
    for (i, (k, v)) in req.headers.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        json_str(&mut out, k);
        out.push(',');
        json_str(&mut out, v);
        out.push(']');
    }
    out.push_str("],\"peer\":");
    json_str(&mut out, &peer.map(|p| p.to_string()).unwrap_or_default());
    out.push('}');
    out
}
