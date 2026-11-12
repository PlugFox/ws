//! Writes the server side of the opening handshake ourselves, so endpoints
//! can craft broken `101` responses and pick subprotocols.

use std::fmt::Write as _;

use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig},
    },
};

use crate::{conn::Conn, http::Request};

/// Limits used by the tungstenite-backed endpoints.
pub fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(256 << 20))
        .max_frame_size(Some(256 << 20))
}

/// `Sec-WebSocket-Accept` for the request's key.
pub fn accept_key(req: &Request) -> String {
    derive_accept_key(
        req.header("sec-websocket-key")
            .unwrap_or("")
            .trim()
            .as_bytes(),
    )
}

/// Subprotocol to answer with: `protocol=` from the query when present
/// (empty means none), else the first one the client offered.
pub fn select_protocol(req: &Request) -> Option<String> {
    match req.query.get("protocol") {
        Some("") => None,
        Some(p) => Some(p.to_owned()),
        None => req
            .header_tokens("sec-websocket-protocol")
            .into_iter()
            .next(),
    }
}

/// The parts of a `101` response; every field can be broken on purpose.
pub struct Head101 {
    /// Header lines in order; `None` values are omitted.
    pub headers: Vec<(String, String)>,
}

impl Head101 {
    /// A correct `101` for `req`.
    pub fn new(req: &Request) -> Self {
        let mut headers = vec![
            ("Upgrade".into(), "websocket".into()),
            ("Connection".into(), "Upgrade".into()),
            ("Sec-WebSocket-Accept".into(), accept_key(req)),
        ];
        if let Some(p) = select_protocol(req) {
            headers.push(("Sec-WebSocket-Protocol".into(), p));
        }
        Head101 { headers }
    }

    /// Replaces (or removes, with `None`) every header named `name`.
    pub fn set(&mut self, name: &str, value: Option<String>) {
        let pos = self
            .headers
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(name));
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        if let Some(v) = value {
            let at = pos.unwrap_or(self.headers.len());
            self.headers
                .insert(at.min(self.headers.len()), (name.to_owned(), v));
        }
    }

    pub fn push(&mut self, name: &str, value: String) {
        self.headers.push((name.to_owned(), value));
    }

    pub fn render(&self) -> Vec<u8> {
        let mut out = String::from("HTTP/1.1 101 Switching Protocols\r\n");
        for (k, v) in &self.headers {
            let _ = write!(out, "{k}: {v}\r\n");
        }
        out.push_str("\r\n");
        out.into_bytes()
    }
}

/// Writes a correct `101` for `req`.
pub async fn upgrade(conn: &mut Conn, req: &Request) -> std::io::Result<()> {
    write_101(conn, &Head101::new(req)).await
}

pub async fn write_101(conn: &mut Conn, head: &Head101) -> std::io::Result<()> {
    conn.write_all(&head.render()).await?;
    conn.flush().await
}

/// Wraps an upgraded connection in tungstenite.
pub async fn websocket(conn: Conn) -> WebSocketStream<Conn> {
    WebSocketStream::from_raw_socket(conn, Role::Server, Some(ws_config())).await
}
