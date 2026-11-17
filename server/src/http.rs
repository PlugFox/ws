//! Minimal HTTP/1.1 front end: reads a request head, parses the query string
//! and writes plain responses. Everything after the head is left to the
//! endpoint, so faults can be scripted byte by byte.

use std::fmt::Write as _;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest request head we accept; bigger heads get `431`.
pub const MAX_HEAD: usize = 16 * 1024;

/// A parsed request head.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The request target exactly as sent (`/echo?x=1`).
    pub target: String,
    /// `HTTP/1.0` or `HTTP/1.1`.
    pub version: String,
    /// Headers in wire order, names as sent.
    pub headers: Vec<(String, String)>,
    /// Path part of the target, percent-decoded.
    pub path: String,
    pub query: Query,
}

impl Request {
    /// First header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// All comma-separated tokens of every header named `name`.
    pub fn header_tokens(&self, name: &str) -> Vec<String> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .flat_map(|(_, v)| v.split(','))
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// True when the request asks for a WebSocket upgrade.
    pub fn is_upgrade(&self) -> bool {
        self.header("sec-websocket-key").is_some()
    }

    /// Returns a copy that targets `target` instead (used by `/delay`).
    /// Query parameters of the original request that the new target does not
    /// set are carried over, except `skip`.
    pub fn retarget(&self, target: &str, skip: &[&str]) -> Request {
        let (path, query) = split_target(target);
        let mut query = Query::parse(query);
        for (k, v) in &self.query.0 {
            if !skip.contains(&k.as_str()) && query.get(k).is_none() {
                query.0.push((k.clone(), v.clone()));
            }
        }
        Request {
            target: target.to_owned(),
            path: percent_decode(path),
            query,
            ..self.clone()
        }
    }
}

/// Why reading a head failed.
#[derive(Debug)]
pub enum HeadError {
    /// The peer closed or the socket failed before a full head arrived.
    Io(std::io::Error),
    /// Head larger than [`MAX_HEAD`].
    TooLarge,
    /// Not parseable as HTTP/1.x.
    Malformed,
}

/// Reads one request head. Returns it together with any bytes the client
/// already sent after it (e.g. a pipelined frame or TLS ClientHello).
pub async fn read_head<S>(io: &mut S) -> Result<(Request, Bytes), HeadError>
where
    S: AsyncRead + Unpin,
{
    let mut buf = BytesMut::with_capacity(2048);
    loop {
        if let Some(end) = find_head_end(&buf) {
            let request = parse_head(&buf[..end]).ok_or(HeadError::Malformed)?;
            let rest = buf.split_off(end).freeze();
            return Ok((request, rest));
        }
        if buf.len() >= MAX_HEAD {
            return Err(HeadError::TooLarge);
        }
        let n = io.read_buf(&mut buf).await.map_err(HeadError::Io)?;
        if n == 0 {
            return Err(HeadError::Io(std::io::ErrorKind::UnexpectedEof.into()));
        }
    }
}

/// Index just past `\r\n\r\n`, if present.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn parse_head(head: &[u8]) -> Option<Request> {
    let mut headers = [httparse::EMPTY_HEADER; 128];
    let mut req = httparse::Request::new(&mut headers);
    if !matches!(req.parse(head), Ok(httparse::Status::Complete(_))) {
        return None;
    }
    let target = req.path?.to_owned();
    let (path, query) = split_target(&target);
    Some(Request {
        method: req.method?.to_owned(),
        version: format!("HTTP/1.{}", req.version?),
        headers: req
            .headers
            .iter()
            .map(|h| {
                (
                    h.name.to_owned(),
                    String::from_utf8_lossy(h.value).into_owned(),
                )
            })
            .collect(),
        path: percent_decode(path),
        query: Query::parse(query),
        target,
    })
}

/// Splits `/path?query` (also accepts absolute-form `ws://host/path?q`).
fn split_target(target: &str) -> (&str, &str) {
    let target = match target.find("://") {
        Some(i) => target[i + 3..]
            .find('/')
            .map_or("/", |j| &target[i + 3 + j..]),
        None => target,
    };
    target.split_once('?').unwrap_or((target, ""))
}

/// Decoded query parameters, in order; keys may repeat.
#[derive(Debug, Clone, Default)]
pub struct Query(pub Vec<(String, String)>);

impl Query {
    pub fn parse(query: &str) -> Query {
        Query(
            query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    (percent_decode(k), percent_decode(v))
                })
                .collect(),
        )
    }

    /// First value of `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of `key`, in order.
    pub fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Value of `key` as a decimal number, or `default`.
    pub fn num(&self, key: &str, default: u64) -> u64 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// `key=1` style flag.
    pub fn flag(&self, key: &str) -> bool {
        self.num(key, 0) != 0
    }

    /// Value of `key`, or `default`.
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }
}

/// Percent-decodes `s` (`+` becomes a space); invalid escapes stay literal.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi << 4 | lo);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Value of one hex digit.
pub fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Decodes a hex string (whitespace ignored); `None` if malformed.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .chunks(2)
        .map(|p| Some(hex(p[0])? << 4 | hex(p[1])?))
        .collect()
}

/// Standard reason phrase for `code` (empty when unknown).
pub fn reason(code: u16) -> &'static str {
    match code {
        101 => "Switching Protocols",
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        407 => "Proxy Authentication Required",
        426 => "Upgrade Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// Renders a complete `Connection: close` response.
pub fn response(code: u16, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {code} {}\r\n", reason(code));
    for (k, v) in headers {
        let _ = write!(head, "{k}: {v}\r\n");
    }
    let _ = write!(
        head,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Writes a response, flushes and half-closes the connection.
pub async fn respond<S>(
    io: &mut S,
    code: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    io.write_all(&response(code, headers, body)).await?;
    io.flush().await?;
    io.shutdown().await
}

/// Writes `s` as a JSON string literal (with quotes) into `out`.
pub fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_query() {
        let q =
            Query::parse("a=1&header=Retry-After%3A5&header=X:y&to=%2Fecho%3Fid%3Dz&s=a+b&bad=%zz");
        assert_eq!(q.get("a"), Some("1"));
        assert_eq!(
            q.all("header").collect::<Vec<_>>(),
            ["Retry-After:5", "X:y"]
        );
        assert_eq!(q.get("to"), Some("/echo?id=z"));
        assert_eq!(q.get("s"), Some("a b"));
        assert_eq!(q.get("bad"), Some("%zz"));
    }

    #[test]
    fn splits_absolute_target() {
        assert_eq!(split_target("ws://h:1/a?b=1"), ("/a", "b=1"));
        assert_eq!(split_target("/a"), ("/a", ""));
    }
}
