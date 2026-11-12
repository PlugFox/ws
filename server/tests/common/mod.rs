//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{self, RootCertStore, pki_types::ServerName},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use ws_bench_server::{Options, Server, start};

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Starts a server on ephemeral ports in the test's runtime.
pub async fn server() -> Server {
    start(&Options::default()).await.expect("server starts")
}

/// Connects a tungstenite client to `ws://127.0.0.1:{port}{path}`.
pub async fn open_ws(port: u16, path: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}{path}");
    connect_async(url).await.expect("ws connect").0
}

/// Like [`ws`] but returns the handshake error instead of panicking.
pub async fn try_ws(port: u16, path: &str) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    connect_async(format!("ws://127.0.0.1:{port}{path}"))
        .await
        .map(|(ws, _)| ws)
}

/// Next data or close message, skipping pings/pongs, with a timeout.
pub async fn next_msg<S>(
    ws: &mut WebSocketStream<S>,
) -> Option<Result<Message, tokio_tungstenite::tungstenite::Error>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("message within 5s");
        match msg {
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            other => return other,
        }
    }
}

/// Sends `msg` and expects it back.
pub async fn roundtrip<S>(ws: &mut WebSocketStream<S>, msg: Message)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    ws.send(msg.clone()).await.unwrap();
    assert_eq!(next_msg(ws).await.unwrap().unwrap(), msg);
}

/// A plain HTTP response.
pub struct HttpResponse {
    pub status: u16,
    pub head: String,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Sends raw request bytes and reads the response until EOF.
pub async fn http_raw(port: u16, request: &str) -> HttpResponse {
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut buf))
        .await
        .expect("response within 5s")
        .unwrap();
    parse_response(&buf)
}

pub fn parse_response(buf: &[u8]) -> HttpResponse {
    let end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(buf.len(), |i| i + 4);
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    HttpResponse {
        status,
        head,
        body: buf[end..].to_vec(),
    }
}

/// `GET path` over plain HTTP.
pub async fn get(port: u16, path: &str) -> HttpResponse {
    http_raw(
        port,
        &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    )
    .await
}

/// A WebSocket upgrade request for `path`.
pub fn upgrade_request(path: &str, extra: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
    )
}

/// Performs the upgrade by hand. Returns the socket, the response head and
/// any bytes received after it.
pub async fn raw_upgrade(port: u16, path: &str) -> (TcpStream, HttpResponse) {
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    tcp.write_all(upgrade_request(path, "").as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        let mut chunk = [0u8; 4096];
        let n = tcp.read(&mut chunk).await.unwrap();
        assert!(n > 0, "EOF before the response head");
        buf.extend_from_slice(&chunk[..n]);
    }
    let resp = parse_response(&buf);
    (tcp, resp)
}

/// Reads until EOF (or error) and returns the bytes plus the error, if any.
pub async fn read_all(tcp: &mut TcpStream, mut buf: Vec<u8>) -> (Vec<u8>, Option<std::io::Error>) {
    let mut chunk = [0u8; 65536];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut chunk))
            .await
            .expect("read within 5s");
        match read {
            Ok(0) => return (buf, None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return (buf, Some(e)),
        }
    }
}

/// Reads exactly `n` bytes.
pub async fn read_n(tcp: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    tokio::time::timeout(Duration::from_secs(5), tcp.read_exact(&mut buf))
        .await
        .expect("read within 5s")
        .unwrap();
    buf
}

/// Fetches `/log?id=` until the server finished the connection.
pub async fn log(port: u16, id: &str) -> serde_json::Value {
    for _ in 0..100 {
        let resp = get(port, &format!("/log?id={id}")).await;
        if resp.status == 200 {
            let json: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
            if json["done"] == true {
                return json;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("log for {id} never finished");
}

/// A TLS client that trusts only `ca_der`.
pub fn tls_connector(ca_der: &[u8]) -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.add(ca_der.to_vec().into()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

/// Opens a TLS connection to `127.0.0.1:port` verifying `name`.
pub async fn tls(ca_der: &[u8], port: u16, name: &str) -> std::io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    let name = ServerName::try_from(name.to_owned()).unwrap();
    tls_connector(ca_der).connect(name, tcp).await
}

/// Builds a client request with extra headers.
pub fn request_with(
    url: &str,
    headers: &[(&'static str, &str)],
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut req = url.into_client_request().unwrap();
    for (k, v) in headers {
        req.headers_mut().insert(*k, v.parse().unwrap());
    }
    req
}

/// Splits unmasked server frames: `(fin, opcode, payload)`.
pub fn split_frames(mut b: &[u8]) -> Vec<(bool, u8, Vec<u8>)> {
    let mut out = Vec::new();
    while b.len() >= 2 {
        let (fin, op) = (b[0] & 0x80 != 0, b[0] & 0x0F);
        let (len, head) = match b[1] & 0x7F {
            126 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
            127 => (
                u64::from_be_bytes(b[2..10].try_into().unwrap()) as usize,
                10,
            ),
            n => (n as usize, 2),
        };
        out.push((fin, op, b[head..head + len].to_vec()));
        b = &b[head + len..];
    }
    out
}

/// A masked client frame.
pub fn client_frame(fin: bool, op: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [9u8, 8, 7, 6];
    let mut out = vec![(fin as u8) << 7 | op];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n => {
            out.push(0x80 | 126);
            out.extend((n as u16).to_be_bytes());
        }
    }
    out.extend(mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}
