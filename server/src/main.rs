//! WebSocket server for benchmarks, tests and examples of the `ws` package.
//!
//! Endpoints (same on the plain and TLS ports):
//!
//! * `/echo` — sends every text/binary message back unchanged.
//! * `/stream?format=json|binary&size=1024&rate=1000&duration=5000&count=N&unicode=0|1`
//!   — server-to-client stream. Every message carries a sequence number and a
//!   send timestamp (microseconds since the Unix epoch) so the client can
//!   measure delivery latency:
//!   * `json`   → text frame `{"seq":N,"ts":T,"items":[...]}`
//!   * `binary` → binary frame, `u64 LE seq` + `u64 LE ts` + filler.
//!
//!   `rate` is messages per second; `rate=0` floods as fast as the socket
//!   allows. The message count is `count`, or `rate * duration / 1000`.
//!   When done the server closes the connection with code 1000, reason `done`.
//!
//! Usage: `ws_bench_server [--host 127.0.0.1] [--port 9090] [--tls-port 9443]`
//! The TLS port uses a freshly generated self-signed certificate for
//! `localhost` / `127.0.0.1`.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use std::fmt::Write as _;

use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = parse_args();
    let host = args
        .get("host")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = args.get("port").map_or(Ok(9090), |v| v.parse())?;
    let tls_port: u16 = args.get("tls-port").map_or(Ok(9443), |v| v.parse())?;

    let plain = TcpListener::bind((host.as_str(), port)).await?;
    let secure = TcpListener::bind((host.as_str(), tls_port)).await?;
    let acceptor = tls_acceptor()?;

    eprintln!("ws  listening on ws://{host}:{port}");
    eprintln!("wss listening on wss://{host}:{tls_port} (self-signed)");

    tokio::spawn(accept_loop(plain, None));
    tokio::spawn(accept_loop(secure, Some(acceptor)));

    tokio::signal::ctrl_c().await?;
    Ok(())
}

fn parse_args() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        if let Some(key) = arg.strip_prefix("--") {
            if let Some((k, v)) = key.split_once('=') {
                map.insert(k.to_owned(), v.to_owned());
            } else if let Some(v) = it.next() {
                map.insert(key.to_owned(), v);
            }
        }
    }
    map
}

fn tls_acceptor() -> Result<TlsAcceptor, BoxError> {
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
    let cert = CertificateDer::from(certified.cert.der().to_vec());
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

async fn accept_loop(listener: TcpListener, tls: Option<TlsAcceptor>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            continue;
        };
        let tls = tls.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(tcp, tls).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

async fn serve(tcp: TcpStream, tls: Option<TlsAcceptor>) -> Result<(), BoxError> {
    tcp.set_nodelay(true)?;
    match tls {
        Some(acceptor) => handle(acceptor.accept(tcp).await?).await,
        None => handle(tcp).await,
    }
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(256 << 20))
        .max_frame_size(Some(256 << 20))
}

#[allow(clippy::result_large_err)] // the handshake callback signature is fixed by tungstenite
async fn handle<S>(stream: S) -> Result<(), BoxError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut uri = None;
    let ws = accept_hdr_async_with_config(
        stream,
        |req: &Request, resp: Response| {
            uri = Some(req.uri().clone());
            Ok(resp)
        },
        Some(ws_config()),
    )
    .await?;
    let uri = uri.ok_or("missing request uri")?;
    let query = parse_query(uri.query().unwrap_or(""));
    match uri.path() {
        "/echo" => echo(ws).await,
        "/stream" => stream_messages(ws, &query).await,
        path => {
            let mut ws = ws;
            ws.close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: format!("unknown path {path}").into(),
            }))
            .await?;
            Ok(())
        }
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

async fn echo<S>(mut ws: WebSocketStream<S>) -> Result<(), BoxError>
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

async fn stream_messages<S>(
    ws: WebSocketStream<S>,
    query: &HashMap<String, String>,
) -> Result<(), BoxError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let get = |k: &str, d: u64| query.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let format = match query.get("format").map(String::as_str) {
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
