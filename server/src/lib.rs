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
//! The binary (`src/main.rs`) runs it standalone; [`start_embedded`] runs it
//! inside another process (the mobile benchmark app) on its own runtime.
//! The TLS port uses a freshly generated self-signed certificate for
//! `localhost` / `127.0.0.1`.

use std::{
    cell::Cell,
    collections::HashMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
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

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Binds the plain and TLS listeners and spawns their accept loops on the
/// current runtime.
pub async fn listen(host: &str, port: u16, tls_port: u16) -> Result<(), BoxError> {
    let plain = TcpListener::bind((host, port)).await?;
    let secure = TcpListener::bind((host, tls_port)).await?;
    let acceptor = tls_acceptor()?;
    tokio::spawn(accept_loop(plain, None));
    tokio::spawn(accept_loop(secure, Some(acceptor)));
    Ok(())
}

static EMBEDDED: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static EMBEDDED_CPU_NANOS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static LAST_CPU_NANOS: Cell<u64> = const { Cell::new(0) };
}

fn thread_cpu_nanos() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid out pointer; CLOCK_THREAD_CPUTIME_ID is supported on
    // Linux/Android and Darwin.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } != 0 {
        return 0;
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Adds the CPU time this thread used since its last report.
fn report_thread_cpu() {
    let now = thread_cpu_nanos();
    let last = LAST_CPU_NANOS.replace(now);
    EMBEDDED_CPU_NANOS.fetch_add(now.saturating_sub(last), Ordering::Relaxed);
}

/// Starts the server on a dedicated runtime with `threads` workers, for
/// running it inside the benchmark process. Idempotent.
pub fn start_embedded(host: &str, port: u16, tls_port: u16, threads: usize) -> Result<(), BoxError> {
    if EMBEDDED.get().is_some() {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads.max(1))
        .thread_name("ws-bench-server")
        .on_thread_start(|| LAST_CPU_NANOS.set(thread_cpu_nanos()))
        .on_thread_park(report_thread_cpu)
        .on_thread_stop(report_thread_cpu)
        .enable_all()
        .build()?;
    runtime.block_on(listen(host, port, tls_port))?;
    let _ = EMBEDDED.set(runtime);
    Ok(())
}

/// CPU time used so far by the embedded server's threads, so the benchmark
/// can subtract it from the process CPU time. Threads report when they park,
/// i.e. as soon as they run out of work.
pub fn embedded_cpu_micros() -> u64 {
    EMBEDDED_CPU_NANOS.load(Ordering::Relaxed) / 1000
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
