//! Client opening handshake (RFC 6455 §4.1), done here instead of by
//! tungstenite so the native transport validates the response exactly like
//! the pure-Dart transport (`lib/src/protocol/handshake.dart`):
//!
//! * a server that selects no subprotocol is accepted (tungstenite fails);
//! * a repeated `Sec-WebSocket-Accept` or `Sec-WebSocket-Protocol`, an
//!   unoffered extension or subprotocol fail;
//! * `Upgrade` / `Connection` are token lists, header names are
//!   case-insensitive.
//!
//! Bytes after the response head are handed to the WebSocket stream.

use std::collections::HashMap;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

/// Response heads larger than this are rejected (as in Dart).
const MAX_HEAD: usize = 64 << 10;

/// What went wrong; mapped to `Closed` by the caller.
#[derive(Debug)]
pub enum Failure {
    /// Not a 101: status, lower-case headers (repeated ones joined by ", ").
    Status(u16, HashMap<String, String>),
    /// A 101 that breaks the rules, or a response that is not HTTP.
    Invalid(String),
    /// The socket failed or closed before the head was complete.
    Io(std::io::Error),
}

pub struct Request<'a> {
    /// Origin-form target: path and query, never empty.
    pub target: &'a str,
    /// `Host` header value (with port when not the default).
    pub host: &'a str,
    pub key: &'a str,
    pub protocols: &'a [String],
    /// Extra headers, already validated by Dart.
    pub headers: &'a [(String, String)],
}

impl Request<'_> {
    pub fn encode(&self) -> Vec<u8> {
        let mut s = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n",
            self.target, self.host, self.key
        );
        if !self.protocols.is_empty() {
            s.push_str("Sec-WebSocket-Protocol: ");
            s.push_str(&self.protocols.join(", "));
            s.push_str("\r\n");
        }
        for (name, value) in self.headers {
            s.push_str(name);
            s.push_str(": ");
            s.push_str(value);
            s.push_str("\r\n");
        }
        s.push_str("\r\n");
        s.into_bytes()
    }
}

/// Sends the request and validates the response. Returns the selected
/// subprotocol and the bytes read after the head.
pub async fn run<S>(
    stream: &mut S,
    request: &Request<'_>,
) -> Result<(Option<String>, Vec<u8>), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream
        .write_all(&request.encode())
        .await
        .map_err(Failure::Io)?;
    stream.flush().await.map_err(Failure::Io)?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(i) = find_head_end(&buf) {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(Failure::Invalid("Handshake response is too large".into()));
        }
        let n = stream.read(&mut chunk).await.map_err(Failure::Io)?;
        if n == 0 {
            return Err(Failure::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Connection closed during the handshake",
            )));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    if end > MAX_HEAD {
        return Err(Failure::Invalid("Handshake response is too large".into()));
    }
    let protocol = validate(&buf[..end], request)?;
    Ok((protocol, buf[end + 4..].to_vec()))
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parses and checks the head (without the final blank line).
pub fn validate(head: &[u8], request: &Request<'_>) -> Result<Option<String>, Failure> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    let status = parts.next().and_then(|s| s.parse::<u16>().ok());
    let (true, Some(status)) = (version.starts_with("HTTP/1."), status) else {
        return Err(Failure::Invalid("Not an HTTP response".into()));
    };
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        let value = value.trim().to_owned();
        *counts.entry(name.clone()).or_default() += 1;
        headers
            .entry(name)
            .and_modify(|v| {
                v.push_str(", ");
                v.push_str(&value);
            })
            .or_insert(value);
    }
    if status != 101 {
        return Err(Failure::Status(status, headers));
    }
    let has_token = |name: &str, token: &str| {
        headers
            .get(name)
            .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    };
    if !has_token("upgrade", "websocket") {
        return Err(Failure::Invalid("Missing \"Upgrade: websocket\"".into()));
    }
    if !has_token("connection", "upgrade") {
        return Err(Failure::Invalid("Missing \"Connection: Upgrade\"".into()));
    }
    let accept = derive_accept_key(request.key.as_bytes());
    if headers.get("sec-websocket-accept").map(String::as_str) != Some(accept.as_str()) {
        return Err(Failure::Invalid("Invalid Sec-WebSocket-Accept".into()));
    }
    if headers
        .get("sec-websocket-extensions")
        .is_some_and(|v| !v.is_empty())
    {
        return Err(Failure::Invalid(
            "Server selected an extension that was not offered".into(),
        ));
    }
    match headers.get("sec-websocket-protocol") {
        None => Ok(None),
        Some(p) if counts["sec-websocket-protocol"] == 1 && request.protocols.contains(p) => {
            Ok(Some(p.clone()))
        }
        Some(p) => Err(Failure::Invalid(format!(
            "Server selected an unknown subprotocol {p}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
    const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

    fn request(protocols: &[String]) -> Request<'_> {
        Request {
            target: "/a?b=1",
            host: "h:8080",
            key: KEY,
            protocols,
            headers: &[],
        }
    }

    fn check(head: &str, protocols: &[String]) -> Result<Option<String>, Failure> {
        validate(head.as_bytes(), &request(protocols))
    }

    fn ok101(extra: &str) -> String {
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {ACCEPT}{extra}"
        )
    }

    #[test]
    fn request_line_and_headers() {
        let protocols = ["v1".to_owned(), "v2".to_owned()];
        let headers = [("X-Token".to_owned(), "t".to_owned())];
        let r = Request {
            headers: &headers,
            ..request(&protocols)
        };
        let text = String::from_utf8(r.encode()).unwrap();
        assert!(text.starts_with("GET /a?b=1 HTTP/1.1\r\nHost: h:8080\r\n"));
        assert!(text.contains("Sec-WebSocket-Protocol: v1, v2\r\n"));
        assert!(text.ends_with("X-Token: t\r\n\r\n"));
    }

    #[test]
    fn accepts_valid_responses() {
        assert_eq!(check(&ok101(""), &[]).unwrap(), None);
        let v1 = ["v1".to_owned()];
        // The server may select none of the offered protocols.
        assert_eq!(check(&ok101(""), &v1).unwrap(), None);
        assert_eq!(
            check(&ok101("\r\nSec-WebSocket-Protocol: v1"), &v1).unwrap(),
            Some("v1".into())
        );
        let lists = format!(
            "HTTP/1.1 101 OK\r\nUPGRADE: foo, WebSocket\r\nconnection: keep-alive, upgrade\r\n\
             sec-websocket-accept: {ACCEPT}"
        );
        assert_eq!(check(&lists, &[]).unwrap(), None);
    }

    #[test]
    fn rejects_broken_responses() {
        let v1 = ["v1".to_owned()];
        for (head, protocols) in [
            (
                ok101("\r\nSec-WebSocket-Accept: x").replace(ACCEPT, "nope"),
                &[][..],
            ),
            (
                ok101(&format!("\r\nSec-WebSocket-Accept: {ACCEPT}")),
                &[][..],
            ),
            (
                ok101("\r\nSec-WebSocket-Extensions: permessage-deflate"),
                &[][..],
            ),
            (ok101("\r\nSec-WebSocket-Protocol: v2"), &v1[..]),
            (ok101("\r\nSec-WebSocket-Protocol: v1"), &[][..]),
            (
                ok101("\r\nSec-WebSocket-Protocol: v1\r\nSec-WebSocket-Protocol: v1"),
                &v1[..],
            ),
            (ok101("").replace("Upgrade: websocket\r\n", ""), &[][..]),
            (ok101("").replace("Connection: Upgrade\r\n", ""), &[][..]),
            ("220 smtp ESMTP".to_owned(), &[][..]),
        ] {
            assert!(
                matches!(check(&head, protocols), Err(Failure::Invalid(_))),
                "{head}"
            );
        }
    }

    #[test]
    fn status_keeps_headers() {
        let r = check("HTTP/1.1 503 Busy\r\nRetry-After: 7\r\nX: a\r\nx: b", &[]);
        let Err(Failure::Status(503, headers)) = r else {
            panic!("{r:?}")
        };
        assert_eq!(headers["retry-after"], "7");
        assert_eq!(headers["x"], "a, b");
    }

    #[tokio::test]
    async fn leftover_bytes_are_returned() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let srv = tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let _ = server.read(&mut buf).await.unwrap();
            let head = ok101("\r\n\r\n");
            server.write_all(head.as_bytes()).await.unwrap();
            server.write_all(&[0x81, 0x00]).await.unwrap();
            server
        });
        let (protocol, rest) = run(&mut client, &request(&[])).await.unwrap();
        assert_eq!(protocol, None);
        // The frame may arrive with the head or later; if with it, it is kept.
        assert!(rest.is_empty() || rest == [0x81, 0x00]);
        drop(srv.await.unwrap());
    }
}
