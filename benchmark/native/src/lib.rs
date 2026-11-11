//! Native (Rust) WebSocket transport for the benchmark — variant "d".
//!
//! A multi-threaded tokio runtime owns the connections. Each connection has
//! a reader task (socket + TLS + framing), a bounded queue, and a decoder
//! task that converts messages and posts them to a Dart `ReceivePort` in
//! batches, so I/O and decoding run on different threads in parallel.
//!
//! Modes:
//! * `0` bytes   — payloads as external typed data (zero-copy), decoded in Dart;
//! * `1` tape    — JSON parsed by simd-json into the tape format;
//! * `2` cobject — JSON parsed into a `Dart_CObject` tree (copied into the
//!   receiving isolate by the VM).
//!
//! Events posted to the port (arrays):
//! * `[0]` open; `[1, closeCode]` closed; `[2, message]` error;
//! * `[3, kind, item, kind, item, ...]` messages, kind: 0 text bytes,
//!   1 binary bytes, 2 tape, 3 cobject tree.

mod dart;
mod tape;

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{
    self, ClientConfig, DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{WebSocketStream, client_async_with_config};

use dart::{DartCObject, DartPort, ExternalBytes, PostCObjectFn};

const MODE_TAPE: u32 = 1;
const MODE_COBJECT: u32 = 2;

const KIND_TEXT: i64 = 0;
const KIND_BINARY: i64 = 1;
const KIND_TAPE: i64 = 2;
const KIND_COBJECT: i64 = 3;

const MAX_BATCH: usize = 64;

enum Command {
    Send(Message),
    Close(u16),
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static CONNECTIONS: OnceLock<Mutex<HashMap<u64, mpsc::UnboundedSender<Command>>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn connections() -> &'static Mutex<HashMap<u64, mpsc::UnboundedSender<Command>>> {
    CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Initializes the runtime. `post` is `NativeApi.postCObject`.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_init(post: PostCObjectFn, threads: u32) -> bool {
    dart::set_post(post);
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads.max(1) as usize)
            .thread_name("ws-native")
            .enable_all()
            .build()
            .expect("tokio runtime")
    });
    true
}

/// Starts the benchmark server inside this process on 127.0.0.1 (for mobile
/// devices, where no separate server process can run). Blocks until bound.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_server_start(port: u16, tls_port: u16, threads: u32) -> bool {
    match ws_bench_server::start_embedded("127.0.0.1", port, tls_port, threads as usize) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("embedded server: {e}");
            false
        }
    }
}

/// CPU time used by the embedded server's threads, in microseconds.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_server_cpu_micros() -> u64 {
    ws_bench_server::embedded_cpu_micros()
}

/// Starts connecting to `url` (NUL-terminated). Returns a connection id.
/// Non-blocking: safe for `@Native(isLeaf: true)`.
///
/// # Safety
/// `url` must be a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_native_connect(url: *const c_char, port: DartPort, mode: u32) -> u64 {
    let Some(runtime) = RUNTIME.get() else {
        return 0;
    };
    let url = unsafe { CStr::from_ptr(url) }
        .to_string_lossy()
        .into_owned();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::unbounded_channel();
    connections().lock().unwrap().insert(id, tx);
    runtime.spawn(async move {
        let code = match connect_and_serve(&url, port, mode, rx).await {
            Ok(code) => code,
            Err(e) => {
                post_error(port, &e.to_string());
                1006
            }
        };
        connections().lock().unwrap().remove(&id);
        post_event(
            port,
            &mut [DartCObject::int(1), DartCObject::int(code as i64)],
        );
    });
    id
}

/// Queues a message. Copies `len` bytes; non-blocking.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_native_send(
    id: u64,
    data: *const u8,
    len: usize,
    binary: bool,
) -> bool {
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let message = if binary {
        Message::Binary(bytes.into())
    } else {
        match String::from_utf8(bytes) {
            Ok(s) => Message::Text(Utf8Bytes::from(s)),
            Err(_) => return false,
        }
    };
    send_command(id, Command::Send(message))
}

/// Starts the closing handshake. Non-blocking.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_close(id: u64, code: u16) -> bool {
    send_command(id, Command::Close(code))
}

fn send_command(id: u64, command: Command) -> bool {
    connections()
        .lock()
        .unwrap()
        .get(&id)
        .is_some_and(|tx| tx.send(command).is_ok())
}

// --- events -------------------------------------------------------------

fn post_event(port: DartPort, items: &mut [DartCObject]) -> bool {
    let mut pointers: Vec<*mut DartCObject> = items.iter_mut().map(|o| o as *mut _).collect();
    let mut array = DartCObject::array(&mut pointers);
    dart::post(port, &mut array)
}

fn post_error(port: DartPort, message: &str) {
    let text = CString::new(message.replace('\0', " ")).unwrap();
    post_event(
        port,
        &mut [DartCObject::int(2), DartCObject::string(text.as_ptr())],
    );
}

enum Item {
    Plain(DartCObject),
    External(ExternalBytes),
    Tree(tape::CObjectTree),
}

/// Converts a batch of messages and posts it as a single port message.
fn post_batch(port: DartPort, mode: u32, batch: &mut Vec<Message>) {
    let mut items: Vec<Item> = Vec::with_capacity(batch.len() * 2 + 1);
    items.push(Item::Plain(DartCObject::int(3)));
    for message in batch.drain(..) {
        match message {
            Message::Text(text) => match mode {
                MODE_TAPE => match tape::build_tape(text.as_bytes().to_vec()) {
                    Ok(t) => {
                        let (ptr, len) = (t.words.as_ptr() as *const u8, t.bytes);
                        items.push(Item::Plain(DartCObject::int(KIND_TAPE)));
                        items.push(Item::External(ExternalBytes::new(t.words, ptr, len, true)));
                    }
                    Err(e) => post_error(port, &e),
                },
                MODE_COBJECT => match tape::build_cobject(text.as_bytes().to_vec()) {
                    Ok(tree) => {
                        items.push(Item::Plain(DartCObject::int(KIND_COBJECT)));
                        items.push(Item::Tree(tree));
                    }
                    Err(e) => post_error(port, &e),
                },
                // MODE_BYTES (0) and anything unknown: raw bytes.
                _ => {
                    let (ptr, len) = (text.as_str().as_ptr(), text.len());
                    items.push(Item::Plain(DartCObject::int(KIND_TEXT)));
                    items.push(Item::External(ExternalBytes::new(text, ptr, len, false)));
                }
            },
            Message::Binary(bytes) => {
                let (ptr, len) = (bytes.as_ptr(), bytes.len());
                items.push(Item::Plain(DartCObject::int(KIND_BINARY)));
                items.push(Item::External(ExternalBytes::new(bytes, ptr, len, false)));
            }
            _ => {}
        }
    }
    let mut pointers: Vec<*mut DartCObject> = items
        .iter_mut()
        .map(|item| match item {
            Item::Plain(o) => o as *mut DartCObject,
            Item::External(e) => &mut e.object as *mut DartCObject,
            Item::Tree(t) => t.root,
        })
        .collect();
    let mut array = DartCObject::array(&mut pointers);
    let posted = dart::post(port, &mut array);
    // External buffers now belong to Dart; free them only if the post failed.
    for item in items {
        if let Item::External(e) = item
            && !posted
        {
            e.release();
        }
    }
}

// --- connection ---------------------------------------------------------

type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn connect_and_serve(
    url: &str,
    port: DartPort,
    mode: u32,
    commands: mpsc::UnboundedReceiver<Command>,
) -> Result<u16, BoxError> {
    let uri: tokio_tungstenite::tungstenite::http::Uri = url.parse()?;
    let secure = uri.scheme_str() == Some("wss");
    let host = uri.host().ok_or("missing host")?.to_owned();
    let tcp_port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), tcp_port)).await?;
    tcp.set_nodelay(true)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(256 << 20))
        .max_frame_size(Some(256 << 20));
    if secure {
        let connector = TlsConnector::from(Arc::new(insecure_tls_config()?));
        let tls = connector.connect(ServerName::try_from(host)?, tcp).await?;
        let (ws, _) = client_async_with_config(url, tls, Some(config)).await?;
        serve(ws, port, mode, commands).await
    } else {
        let (ws, _) = client_async_with_config(url, tcp, Some(config)).await?;
        serve(ws, port, mode, commands).await
    }
}

async fn serve<S>(
    ws: WebSocketStream<S>,
    port: DartPort,
    mode: u32,
    mut commands: mpsc::UnboundedReceiver<Command>,
) -> Result<u16, BoxError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    post_event(port, &mut [DartCObject::int(0)]);
    let (mut sink, mut stream) = ws.split();

    // Decoder: runs on another worker thread, preserves message order.
    let (frames, mut frames_rx) = mpsc::channel::<Message>(1024);
    let decoder = tokio::spawn(async move {
        let mut batch = Vec::with_capacity(MAX_BATCH);
        while let Some(first) = frames_rx.recv().await {
            batch.push(first);
            while batch.len() < MAX_BATCH {
                match frames_rx.try_recv() {
                    Ok(m) => batch.push(m),
                    Err(_) => break,
                }
            }
            post_batch(port, mode, &mut batch);
        }
    });

    // Writer: outgoing messages and the closing handshake. Stops when the
    // reader is done (`stop`) and hands the sink back for the final flush.
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Send(m)) => {
                        if sink.send(m).await.is_err() {
                            break;
                        }
                    }
                    Some(Command::Close(code)) => {
                        let frame = CloseFrame { code: code.into(), reason: "".into() };
                        let _ = sink.send(Message::Close(Some(frame))).await;
                    }
                    None => break,
                },
                _ = &mut stopped => break,
            }
        }
        sink
    });

    let mut code = 1006u16;
    while let Some(message) = stream.next().await {
        match message {
            Ok(m @ (Message::Text(_) | Message::Binary(_))) => {
                if frames.send(m).await.is_err() {
                    break;
                }
            }
            Ok(Message::Close(frame)) => {
                code = frame.map_or(1005, |f| f.code.into());
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    drop(frames);
    let _ = decoder.await;
    let _ = stop.send(());
    if let Ok(mut sink) = writer.await {
        let _ = sink.close().await;
    }
    Ok(code)
}

// --- TLS (benchmark only: accepts any certificate) -----------------------

#[derive(Debug)]
struct AcceptAnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn insecure_tls_config() -> Result<ClientConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate(provider)))
        .with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use super::tape::build_tape;

    fn words(json: &str) -> Vec<u64> {
        let t = build_tape(json.as_bytes().to_vec()).unwrap();
        let n = (t.words[0] & 0xFFFF_FFFF) as usize;
        t.words[1..=n].to_vec()
    }

    #[test]
    fn tape_layout() {
        // {"a":[1,true],"b":{}}
        let w = words(r#"{"a":[1,true],"b":{}}"#);
        assert_eq!(w[0] >> 56, 0x7B);
        assert_eq!(w[0] & 0xFF_FFFF_FFFF_FFFF, w.len() as u64); // skip = end
        assert_eq!(w[1], 2); // fields
        assert_eq!(w[2] >> 56, 0x73); // key "a"
        assert_eq!(w[4] >> 56, 0x5B); // array
        assert_eq!(w[5], 2);
        assert_eq!(w[6] >> 56, 0x6C);
        assert_eq!(w[7], 1);
        assert_eq!(w[8] >> 56, 0x74);
        assert_eq!(w[4] & 0xFF_FFFF_FFFF_FFFF, 9); // array skip
    }
}
