//! Request routing: reads the HTTP head, then dispatches on the path.

pub mod bench;
pub mod handshake_faults;
pub mod raw;
pub mod scripted;

use std::{sync::Arc, time::Duration};

use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::{
    BoxError, Shared,
    conn::Conn,
    frame::{End, Session},
    handshake,
    http::{self, HeadError, Request},
};

use handshake_faults as hs;

/// Serves one connection from its first byte.
pub async fn serve(mut conn: Conn, shared: Arc<Shared>) -> Result<(), BoxError> {
    let (mut req, rest) = match http::read_head(conn.inner()).await {
        Ok(head) => head,
        Err(HeadError::TooLarge) => return reject(conn, 431).await,
        Err(HeadError::Malformed) => return reject(conn, 400).await,
        Err(HeadError::Io(_)) => return Ok(()),
    };
    conn.set_prefix(rest);
    // `/delay?ms=&then=<target>` re-routes after sleeping.
    while req.path == "/delay" {
        tokio::time::sleep(Duration::from_millis(req.query.num("ms", 0))).await;
        let then = req.query.str_or("then", "/echo").to_owned();
        req = req.retarget(&then, &["ms", "then"]);
    }
    if let Some(id) = req.query.get("id")
        && !matches!(req.path.as_str(), "/log" | "/proxy-log" | "/ca.pem")
    {
        conn.set_log(shared.logs.open(id));
    }
    route(conn, &req, &shared).await
}

/// Answers a bad request head, then reads what the client is still
/// sending so the close is a FIN, not an RST that could eat the response.
async fn reject(mut conn: Conn, code: u16) -> Result<(), BoxError> {
    http::respond(&mut conn, code, &[], b"").await?;
    Session::new(conn).finish(End::Fin).await;
    Ok(())
}

/// Paths that answer with plain HTTP whatever the request looks like.
async fn route(mut conn: Conn, req: &Request, shared: &Shared) -> Result<(), BoxError> {
    match req.path.as_str() {
        "/ca.pem" => {
            http::respond(
                &mut conn,
                200,
                &[("Content-Type", "application/x-pem-file")],
                shared.ca_pem.as_bytes(),
            )
            .await?
        }
        "/log" => log(conn, req, shared).await?,
        "/proxy-log" => json(conn, shared.logs.tunnels_json()).await?,
        "/status" => hs::status(conn, req).await?,
        "/redirect" => hs::redirect(conn, req).await?,
        "/redirect-loop" => hs::redirect_loop(conn, req).await?,
        "/hang" => hs::hang(conn).await,
        "/eof-handshake" => hs::eof_handshake(conn, req).await?,
        "/junk" => hs::junk(conn).await?,
        _ if !req.is_upgrade() => not_websocket(conn, req).await?,
        _ => websocket(conn, req).await?,
    }
    Ok(())
}

/// Endpoints that need a WebSocket request.
async fn websocket(mut conn: Conn, req: &Request) -> Result<(), BoxError> {
    let q = &req.query;
    match req.path.as_str() {
        "/big-head" => return hs::big_head(conn, req).await,
        "/early-hints" => return hs::early_hints(conn, req).await,
        "/bad-101" => return hs::bad_101(conn, req).await,
        "/headers" => return hs::headers(conn, req).await,
        _ => {}
    }
    // `/raw` validates its script before upgrading, so typos give a 400.
    let script = match req.path.as_str() {
        "/raw" => match raw::script(q) {
            Ok(s) => Some(s),
            Err(msg) => return Ok(http::respond(&mut conn, 400, &[], msg.as_bytes()).await?),
        },
        _ => None,
    };
    handshake::upgrade(&mut conn, req).await?;
    match req.path.as_str() {
        "/echo" => bench::echo_conn(handshake::websocket(conn).await).await?,
        "/stream" => bench::stream_messages(handshake::websocket(conn).await, q).await?,
        "/close" => scripted::close(Session::new(conn), q, false).await?,
        "/close-empty" => scripted::close(Session::new(conn), q, true).await?,
        "/drop" => scripted::drop_conn(Session::new(conn), q).await?,
        "/ignore-close" => scripted::ignore_close(Session::new(conn)).await?,
        "/silent" => scripted::silent(Session::new(conn), q).await,
        "/slow-read" => scripted::slow_read(Session::new(conn), q).await,
        "/flood" => scripted::flood(Session::new(conn), q).await?,
        "/ping" => scripted::ping(Session::new(conn), q).await?,
        "/pong" => scripted::pong(Session::new(conn), q).await?,
        "/stall" => scripted::stall(Session::new(conn), q).await?,
        "/fragment" => scripted::fragment(Session::new(conn), q).await?,
        "/raw" => raw::raw(Session::new(conn), q, script.unwrap()).await?,
        path => {
            let ws = handshake::websocket(conn).await;
            bench::close_with(ws, CloseCode::Policy, format!("unknown path {path}")).await?
        }
    }
    Ok(())
}

/// A plain request to a WebSocket path: `426`, or `404` for unknown paths.
async fn not_websocket(mut conn: Conn, req: &Request) -> std::io::Result<()> {
    const KNOWN: &[&str] = &[
        "/echo",
        "/stream",
        "/big-head",
        "/early-hints",
        "/bad-101",
        "/headers",
        "/close",
        "/close-empty",
        "/drop",
        "/ignore-close",
        "/silent",
        "/slow-read",
        "/flood",
        "/ping",
        "/pong",
        "/stall",
        "/fragment",
        "/raw",
    ];
    if KNOWN.contains(&req.path.as_str()) {
        let msg = b"this endpoint needs a WebSocket upgrade";
        http::respond(&mut conn, 426, &[("Upgrade", "websocket")], msg).await
    } else {
        http::respond(&mut conn, 404, &[], b"not found").await
    }
}

/// `/log?id=`: what connection `id` sent, or `404`.
async fn log(mut conn: Conn, req: &Request, shared: &Shared) -> std::io::Result<()> {
    match shared.logs.conn_json(req.query.str_or("id", "")) {
        Some(body) => json(conn, body).await,
        None => http::respond(&mut conn, 404, &[], b"unknown id").await,
    }
}

async fn json(mut conn: Conn, body: String) -> std::io::Result<()> {
    let headers = [("Content-Type", "application/json")];
    http::respond(&mut conn, 200, &headers, body.as_bytes()).await
}
