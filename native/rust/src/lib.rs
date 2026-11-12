//! Native WebSocket transport for the Dart package `ws` (package
//! `ws_native`). See `docs/DECISIONS.md` D8–D10, D17, D18 in the repository.
//!
//! A multi-threaded tokio runtime owns all connections. Per connection:
//! * a reader task: socket + TLS + framing (tungstenite), receive credits;
//! * a decoder task: turns messages into batches (raw bytes or a JSON tape)
//!   and posts them to the Dart `ReceivePort` as external typed data;
//! * a writer task: outgoing messages, pings, the closing handshake;
//! * a probe: posts `null` every 30 s so a dead isolate (hot restart) is
//!   detected even on an idle connection.
//!
//! The ABI is struct-free: scalars, pointers to bytes, and one JSON config
//! string for `connect`. Every entry point catches panics.
//!
//! Events posted to Dart (arrays, first element = event code):
//! * `[0, protocol | null]` — open;
//! * `[1, code | null, reason, error_kind, message | null, http_status | null,
//!   headers_json | null]` — closed (always the last event);
//! * `[3, bytes, kind, item, kind, item, ...]` — messages; kind 0 = UTF-8
//!   text bytes, 1 = binary, 2 = JSON tape; `bytes` = payload bytes to grant
//!   back as credits;
//! * `[4]` — pong; `[5]` — drain (after `ws_native_request_drain`);
//! * `null` — liveness probe, ignored.

mod dart;
mod handshake;
mod net;
mod tape;
mod tls;

use std::collections::HashMap;
use std::ffi::{CString, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{
    self as tungstenite, Message, Utf8Bytes,
    error::{CapacityError, ProtocolError},
    handshake::client::generate_key,
    http::{Uri, uri::InvalidUri},
    protocol::{CloseFrame, Role, WebSocketConfig, frame::coding::CloseCode},
};

use dart::{DartCObject, DartPort, ExternalBytes, PostCObjectFn};

/// Bumped on any change of the functions or events below.
const ABI_VERSION: u32 = 3;

const EV_OPEN: i64 = 0;
const EV_CLOSED: i64 = 1;
const EV_BATCH: i64 = 3;
const EV_PONG: i64 = 4;
const EV_DRAIN: i64 = 5;

const KIND_TEXT: i64 = 0;
const KIND_BINARY: i64 = 1;
const KIND_TAPE: i64 = 2;

const ERR_NONE: i64 = 0;
const ERR_CONNECTION: i64 = 1;
const ERR_HANDSHAKE: i64 = 2;
const ERR_PROTOCOL: i64 = 3;
const ERR_TLS: i64 = 4;
const ERR_ABORTED: i64 = 5;

const SEND_OK: u32 = 0;
const SEND_CLOSED: u32 = 1;
const SEND_INVALID_UTF8: u32 = 2;

const MAX_BATCH: usize = 64;
const PROBE_INTERVAL: Duration = Duration::from_secs(30);

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static CONNECTIONS: OnceLock<Mutex<HashMap<u64, Arc<Shared>>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn connections() -> &'static Mutex<HashMap<u64, Arc<Shared>>> {
    CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn table() -> std::sync::MutexGuard<'static, HashMap<u64, Arc<Shared>>> {
    // A panic while holding the lock must not disable every connection.
    connections()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn connection(id: u64) -> Option<Arc<Shared>> {
    table().get(&id).cloned()
}

/// Runs `f`, turning a panic into `fallback` (never unwinds into Dart).
fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

enum Command {
    Send(Message, i64),
    Ping,
    Close(Option<u16>, String),
}

/// State shared between Dart calls and the connection's tasks.
struct Shared {
    port: DartPort,
    commands: mpsc::UnboundedSender<Command>,
    /// Bytes queued for sending, not yet written.
    buffered: AtomicI64,
    drain_requested: AtomicBool,
    /// Receive window in bytes; the reader waits while it is ≤ 0.
    credits: AtomicI64,
    credit_notify: Notify,
    aborted: AtomicBool,
    abort_notify: Notify,
    /// Set once the handshake succeeded; sends before that are rejected.
    open: AtomicBool,
}

impl Shared {
    fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        self.abort_notify.notify_waiters();
        self.abort_notify.notify_one();
    }

    fn post(&self, items: &mut [DartCObject]) -> bool {
        let ok = post_event(self.port, items);
        if !ok {
            // The isolate is gone (hot restart, shutdown): stop.
            self.abort();
        }
        ok
    }

    fn maybe_post_drain(&self) {
        if self.buffered.load(Ordering::SeqCst) == 0
            && self.drain_requested.swap(false, Ordering::SeqCst)
        {
            self.post(&mut [DartCObject::int(EV_DRAIN)]);
        }
    }
}

/// Unknown fields are an error: a library older than its Dart side must
/// not silently ignore a setting (a proxy, say) the caller relies on.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    url: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    protocols: Vec<String>,
    #[serde(default = "default_max_message")]
    max_message_size: usize,
    #[serde(default)]
    json: bool,
    #[serde(default = "default_window")]
    window: i64,
    #[serde(default)]
    allow_insecure: bool,
    #[serde(default)]
    roots_pem: Option<String>,
    #[serde(default)]
    proxy: Option<net::Proxy>,
    /// TCP keepalive idle time in milliseconds; absent disables it.
    #[serde(default)]
    keepalive_ms: Option<u64>,
}

fn default_max_message() -> usize {
    64 << 20
}

/// Credits charged per message on top of its payload: the bookkeeping
/// (channel slot, `Dart_CObject`, external typed data) costs memory even
/// for empty messages (docs/review/native.md, finding 3).
const MESSAGE_OVERHEAD: i64 = 64;

fn charge(len: usize) -> i64 {
    len as i64 + MESSAGE_OVERHEAD
}

fn default_window() -> i64 {
    8 << 20
}

// --- entry points ------------------------------------------------------------

/// ABI version; Dart checks it once before using anything else.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_abi_version() -> u32 {
    ABI_VERSION
}

/// Initializes the runtime (idempotent). `post` is `NativeApi.postCObject`.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_init(post: Option<PostCObjectFn>, threads: u32) -> bool {
    guard(false, || {
        let Some(post) = post else {
            return false;
        };
        dart::set_post(post);
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(threads.clamp(1, 16) as usize)
                .thread_name("ws-native")
                .enable_all()
                .build()
                .expect("tokio runtime")
        });
        true
    })
}

/// Allocates a `len`-byte buffer for Dart to fill and hand over to
/// `ws_native_connect`, `ws_native_send` or `ws_native_close`, which take
/// ownership. Dart calls the library through function pointers, where
/// passing a pointer into a Dart `Uint8List` is not allowed, so the one copy
/// happens on the Dart side instead of here (DECISIONS.md D30). Returns
/// null on failure.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_alloc(len: usize) -> *mut u8 {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::<[u8]>::new_uninit_slice(len)).cast::<u8>()
    })
}

/// Frees a buffer from `ws_native_alloc` that was not handed over.
///
/// # Safety
/// `data` must come from `ws_native_alloc(len)` and not be handed over.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_native_free(data: *mut u8, len: usize) {
    if !data.is_null() {
        drop(unsafe { take_buffer(data, len) });
    }
}

/// Takes ownership of a buffer from `ws_native_alloc(len)` that Dart has
/// filled completely.
unsafe fn take_buffer(data: *mut u8, len: usize) -> Vec<u8> {
    if data.is_null() {
        return Vec::new();
    }
    unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) }.into_vec()
}

/// Starts connecting. `config` is UTF-8 JSON (see `Config`). Returns a
/// connection id, or 0 if the runtime is not initialized or the config is
/// invalid. All further events go to `port`. Non-blocking.
///
/// # Safety
/// `config` must come from `ws_native_alloc(len)`, filled; this call takes
/// ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_native_connect(config: *mut u8, len: usize, port: DartPort) -> u64 {
    let bytes = unsafe { take_buffer(config, len) };
    guard(0, move || {
        let Some(runtime) = RUNTIME.get() else {
            return 0;
        };
        let Ok(config) = serde_json::from_slice::<Config>(&bytes) else {
            return 0;
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            port,
            commands: tx,
            buffered: AtomicI64::new(0),
            drain_requested: AtomicBool::new(false),
            credits: AtomicI64::new(config.window.max(1)),
            credit_notify: Notify::new(),
            aborted: AtomicBool::new(false),
            abort_notify: Notify::new(),
            open: AtomicBool::new(false),
        });
        table().insert(id, shared.clone());
        let task = runtime.spawn(run(shared.clone(), config, rx));
        runtime.spawn(async move {
            // A panic inside the connection still produces the final event.
            let outcome = match task.await {
                Ok(outcome) => outcome,
                Err(_) => Closed::error(ERR_CONNECTION, "internal error (panic)".into()),
            };
            table().remove(&id);
            outcome.post(shared.port);
        });
        id
    })
}

/// Queues a message (`binary` = false: text, must be UTF-8). Returns 0 ok,
/// 1 closed, 2 invalid UTF-8. Non-blocking.
///
/// # Safety
/// `data` must come from `ws_native_alloc(len)`, filled; this call takes
/// ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_native_send(id: u64, data: *mut u8, len: usize, binary: bool) -> u32 {
    let bytes = unsafe { take_buffer(data, len) };
    guard(SEND_CLOSED, move || {
        let Some(shared) = connection(id) else {
            return SEND_CLOSED;
        };
        if !shared.open.load(Ordering::SeqCst) || shared.aborted.load(Ordering::SeqCst) {
            return SEND_CLOSED;
        }
        let message = if binary {
            Message::Binary(bytes.into())
        } else {
            match String::from_utf8(bytes) {
                Ok(s) => Message::Text(Utf8Bytes::from(s)),
                Err(_) => return SEND_INVALID_UTF8,
            }
        };
        shared.buffered.fetch_add(len as i64, Ordering::SeqCst);
        if shared
            .commands
            .send(Command::Send(message, len as i64))
            .is_err()
        {
            shared.buffered.fetch_sub(len as i64, Ordering::SeqCst);
            return SEND_CLOSED;
        }
        SEND_OK
    })
}

/// Sends a protocol ping.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_ping(id: u64) -> bool {
    guard(false, || {
        connection(id).is_some_and(|s| s.commands.send(Command::Ping).is_ok())
    })
}

/// Starts the closing handshake. `code` < 0 sends no status code.
///
/// # Safety
/// `reason` must come from `ws_native_alloc(reason_len)`, filled with UTF-8;
/// this call takes ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ws_native_close(
    id: u64,
    code: i32,
    reason: *mut u8,
    reason_len: usize,
) -> bool {
    let reason = unsafe { take_buffer(reason, reason_len) };
    guard(false, move || {
        let reason = String::from_utf8_lossy(&reason).into_owned();
        let code = u16::try_from(code).ok();
        connection(id).is_some_and(|s| s.commands.send(Command::Close(code, reason)).is_ok())
    })
}

/// Drops the connection without a closing handshake. The closed event
/// still follows.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_abort(id: u64) -> bool {
    guard(false, || match connection(id) {
        Some(shared) => {
            shared.abort();
            true
        }
        None => false,
    })
}

/// `NativeFinalizer` callback: the Dart connection object was collected (or
/// its isolate died) — abort. `token` is the connection id.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_finalize(token: *mut c_void) {
    let _ = ws_native_abort(token as u64);
}

/// Returns `bytes` of receive window to the connection.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_grant(id: u64, bytes: i64) -> bool {
    guard(false, || match connection(id) {
        Some(shared) => {
            shared.credits.fetch_add(bytes, Ordering::SeqCst);
            shared.credit_notify.notify_one();
            true
        }
        None => false,
    })
}

/// Bytes queued but not yet written to the socket.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_buffered(id: u64) -> i64 {
    guard(0, || {
        connection(id).map_or(0, |s| s.buffered.load(Ordering::SeqCst))
    })
}

/// Asks for one drain event when the send queue is empty.
#[unsafe(no_mangle)]
pub extern "C" fn ws_native_request_drain(id: u64) -> bool {
    guard(false, || match connection(id) {
        Some(shared) => {
            shared.drain_requested.store(true, Ordering::SeqCst);
            shared.maybe_post_drain();
            true
        }
        None => false,
    })
}

// --- events -------------------------------------------------------------------

fn post_event(port: DartPort, items: &mut [DartCObject]) -> bool {
    let mut pointers: Vec<*mut DartCObject> = items.iter_mut().map(|o| o as *mut _).collect();
    let mut array = DartCObject::array(&mut pointers);
    dart::post(port, &mut array)
}

/// The final event of a connection.
struct Closed {
    code: Option<u16>,
    reason: String,
    kind: i64,
    message: Option<String>,
    status: Option<u16>,
    headers: Option<String>,
}

impl Closed {
    fn error(kind: i64, message: String) -> Self {
        Self {
            code: None,
            reason: String::new(),
            kind,
            message: Some(message),
            status: None,
            headers: None,
        }
    }

    fn post(self, port: DartPort) {
        let c = |s: String| CString::new(s.replace('\0', " ")).unwrap_or_default();
        let reason = c(self.reason);
        let message = self.message.map(c);
        let headers = self.headers.map(c);
        let string_or_null = |s: &Option<CString>| match s {
            Some(s) => DartCObject::string(s.as_ptr()),
            None => DartCObject::null(),
        };
        let int_or_null =
            |v: Option<u16>| v.map_or(DartCObject::null(), |v| DartCObject::int(v as i64));
        post_event(
            port,
            &mut [
                DartCObject::int(EV_CLOSED),
                int_or_null(self.code),
                DartCObject::string(reason.as_ptr()),
                DartCObject::int(self.kind),
                string_or_null(&message),
                int_or_null(self.status),
                string_or_null(&headers),
            ],
        );
    }
}

fn map_error(error: tungstenite::Error) -> Closed {
    use tungstenite::Error as E;
    match error {
        E::Http(response) => {
            let status = response.status().as_u16();
            let headers: HashMap<String, String> = response
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_owned(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect();
            Closed {
                code: None,
                reason: String::new(),
                kind: ERR_HANDSHAKE,
                message: Some(format!("Unexpected HTTP status {status}")),
                status: Some(status),
                headers: serde_json::to_string(&headers).ok(),
            }
        }
        E::Capacity(CapacityError::MessageTooLong { .. }) => Closed {
            code: Some(1009),
            reason: "Message too big".into(),
            kind: ERR_PROTOCOL,
            message: Some("Message too big".into()),
            status: None,
            headers: None,
        },
        E::Utf8(_) => Closed {
            code: Some(1007),
            reason: "Invalid UTF-8".into(),
            kind: ERR_PROTOCOL,
            message: Some("Invalid UTF-8".into()),
            status: None,
            headers: None,
        },
        E::Protocol(ProtocolError::HandshakeIncomplete) | E::Url(_) | E::HttpFormat(_) => {
            Closed::error(ERR_HANDSHAKE, error.to_string())
        }
        // EOF without a close frame: the most common disconnect (proxy idle
        // timeout, server crash), not a protocol violation.
        E::Protocol(ProtocolError::ResetWithoutClosingHandshake) => Closed::error(
            ERR_CONNECTION,
            "Connection closed without a close frame".into(),
        ),
        E::Protocol(p) => Closed {
            code: Some(1002),
            reason: "Protocol error".into(),
            kind: ERR_PROTOCOL,
            message: Some(p.to_string()),
            status: None,
            headers: None,
        },
        E::Tls(_) => Closed::error(ERR_TLS, error.to_string()),
        other => Closed::error(ERR_CONNECTION, other.to_string()),
    }
}

// --- connection ---------------------------------------------------------------

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn run(
    shared: Arc<Shared>,
    config: Config,
    mut commands: mpsc::UnboundedReceiver<Command>,
) -> Closed {
    let connecting = open(&config);
    tokio::pin!(connecting);
    let (ws, protocol) = loop {
        tokio::select! {
            result = &mut connecting => match result {
                Ok(ok) => break ok,
                Err(closed) => return closed,
            },
            _ = shared.abort_notify.notified() => {
                return Closed::error(ERR_ABORTED, "Connection aborted".into());
            }
            command = commands.recv() => match command {
                // close() before open: nothing to close politely.
                Some(Command::Close(..)) | None => {
                    return Closed::error(ERR_ABORTED, "Connection aborted".into());
                }
                Some(_) => {}
            },
        }
        if shared.aborted.load(Ordering::SeqCst) {
            return Closed::error(ERR_ABORTED, "Connection aborted".into());
        }
    };
    if shared.aborted.load(Ordering::SeqCst) {
        return Closed::error(ERR_ABORTED, "Connection aborted".into());
    }
    let protocol = protocol.and_then(|p| CString::new(p).ok());
    shared.open.store(true, Ordering::SeqCst);
    if !shared.post(&mut [
        DartCObject::int(EV_OPEN),
        match &protocol {
            Some(p) => DartCObject::string(p.as_ptr()),
            None => DartCObject::null(),
        },
    ]) {
        return Closed::error(ERR_ABORTED, "Isolate is gone".into());
    }
    serve(ws, shared, config.json, commands).await
}

async fn open(config: &Config) -> Result<(WebSocketStream<Box<dyn Io>>, Option<String>), Closed> {
    let connection = |e: BoxError| Closed::error(ERR_CONNECTION, e.to_string());
    let uri: Uri = config
        .url
        .parse()
        .map_err(|e: InvalidUri| Closed::error(ERR_HANDSHAKE, e.to_string()))?;
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => {
            return Err(Closed::error(
                ERR_HANDSHAKE,
                "Expected a ws:// or wss:// URL".into(),
            ));
        }
    };
    let authority_host = uri
        .host()
        .ok_or_else(|| Closed::error(ERR_HANDSHAKE, "URL has no host".into()))?;
    let host = authority_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let default_port = if secure { 443 } else { 80 };
    let port = uri.port_u16().unwrap_or(default_port);
    let host_header = if port == default_port {
        authority_host.to_owned()
    } else {
        format!("{authority_host}:{port}")
    };
    let target = uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let target = if target.starts_with('/') {
        target
    } else {
        format!("/{target}")
    };

    let tcp = net::connect(
        &host,
        port,
        config.proxy.as_ref(),
        config.keepalive_ms.map(Duration::from_millis),
    )
    .await
    .map_err(|e| connection(e.into()))?;
    let mut stream: Box<dyn Io> = if secure {
        let tls = tls::client_config(config.allow_insecure, config.roots_pem.as_deref())
            .map_err(|e| Closed::error(ERR_TLS, e.to_string()))?;
        let name = ServerName::try_from(host).map_err(|e| Closed::error(ERR_TLS, e.to_string()))?;
        let stream = TlsConnector::from(tls)
            .connect(name, tcp)
            .await
            .map_err(|e| Closed::error(ERR_TLS, e.to_string()))?;
        Box::new(stream)
    } else {
        Box::new(tcp)
    };

    let key = generate_key();
    let request = handshake::Request {
        target: &target,
        host: &host_header,
        key: &key,
        protocols: &config.protocols,
        headers: &config.headers,
    };
    let (protocol, leftover) = handshake::run(&mut stream, &request)
        .await
        .map_err(|failure| match failure {
            handshake::Failure::Status(status, headers) => Closed {
                code: None,
                reason: String::new(),
                kind: ERR_HANDSHAKE,
                message: Some(format!("Unexpected HTTP status {status}")),
                status: Some(status),
                headers: serde_json::to_string(&headers).ok(),
            },
            handshake::Failure::Invalid(message) => Closed::error(ERR_HANDSHAKE, message),
            handshake::Failure::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                Closed::error(ERR_HANDSHAKE, e.to_string())
            }
            handshake::Failure::Io(e) => Closed::error(ERR_CONNECTION, e.to_string()),
        })?;
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(config.max_message_size))
        .max_frame_size(Some(config.max_message_size));
    let ws =
        WebSocketStream::from_partially_read(stream, leftover, Role::Client, Some(ws_config)).await;
    Ok((ws, protocol))
}

/// Aborts the connection's helper tasks when `serve` ends in any way,
/// including a panic (they hold `Arc<Shared>`, which would otherwise keep
/// them alive forever).
struct AbortOnDrop(Vec<tokio::task::AbortHandle>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

async fn serve(
    ws: WebSocketStream<Box<dyn Io>>,
    shared: Arc<Shared>,
    json: bool,
    mut commands: mpsc::UnboundedReceiver<Command>,
) -> Closed {
    let (mut sink, mut stream) = ws.split();

    // Decoder: another worker thread; keeps message order.
    let (frames, mut frames_rx) = mpsc::channel::<Message>(1024);
    let decoder_shared = shared.clone();
    let decoder = tokio::spawn(async move {
        let mut batch = Vec::with_capacity(MAX_BATCH);
        while let Some(first) = frames_rx.recv().await {
            // After abort nothing more is delivered (DECISIONS.md D27).
            if decoder_shared.aborted.load(Ordering::SeqCst) {
                return;
            }
            batch.push(first);
            while batch.len() < MAX_BATCH {
                match frames_rx.try_recv() {
                    Ok(m) => batch.push(m),
                    Err(_) => break,
                }
            }
            if !post_batch(&decoder_shared, json, &mut batch) {
                return;
            }
        }
    });

    // Writer. `buffered` counts bytes not yet handed to the OS. A message's
    // bytes are subtracted once the next message was accepted (the sink
    // has written everything before it then, apart from at most
    // tungstenite's write buffer of 128 KiB) or after a flush. Under
    // sustained load the count therefore falls steadily, which the
    // heartbeat reads as upload progress (DECISIONS.md D41).
    let writer_shared = shared.clone();
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let mut unflushed = 0i64;
        loop {
            let command = tokio::select! {
                // Commands first: a queued close must go out before stopping.
                biased;
                command = commands.recv() => match command {
                    Some(command) => command,
                    None => break,
                },
                _ = &mut stopped => break,
            };
            let result = match command {
                Command::Send(m, len) => {
                    let result = sink.feed(m).await;
                    if result.is_ok() && unflushed > 0 {
                        writer_shared
                            .buffered
                            .fetch_sub(unflushed, Ordering::SeqCst);
                        unflushed = 0;
                    }
                    unflushed += len;
                    result
                }
                Command::Ping => sink.feed(Message::Ping(Default::default())).await,
                Command::Close(code, reason) => {
                    let frame = code.map(|code| CloseFrame {
                        code: CloseCode::from(code),
                        reason: reason.into(),
                    });
                    sink.feed(Message::Close(frame)).await
                }
            };
            // Flush once the queue is empty: batches writes under load.
            let result = match result {
                Ok(()) if commands.is_empty() => sink.flush().await,
                other => other,
            };
            if result.is_ok() && commands.is_empty() {
                writer_shared
                    .buffered
                    .fetch_sub(unflushed, Ordering::SeqCst);
                unflushed = 0;
                writer_shared.maybe_post_drain();
            }
            if result.is_err() {
                break;
            }
        }
        // Whatever is left will never be sent: keep the counter honest.
        commands.close();
        while let Ok(command) = commands.try_recv() {
            if let Command::Send(_, len) = command {
                unflushed += len;
            }
        }
        writer_shared
            .buffered
            .fetch_sub(unflushed, Ordering::SeqCst);
        writer_shared.maybe_post_drain();
        sink
    });

    // Liveness probe for idle connections of a dead isolate.
    let probe_shared = shared.clone();
    let probe = tokio::spawn(async move {
        let mut interval = tokio::time::interval(PROBE_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            if !probe_shared.post(&mut [DartCObject::null()]) {
                return;
            }
        }
    });
    let _guard = AbortOnDrop(vec![
        decoder.abort_handle(),
        writer.abort_handle(),
        probe.abort_handle(),
    ]);

    let mut outcome = Closed::error(
        ERR_CONNECTION,
        "Connection closed without a close frame".into(),
    );
    // A close frame we owe the server after a protocol error (sent after the
    // writer is stopped, so it cannot be lost in the command queue).
    let mut owed_close: Option<CloseFrame> = None;
    loop {
        // Wait for receive credits (flow control).
        loop {
            let notified = shared.credit_notify.notified();
            if shared.credits.load(Ordering::SeqCst) > 0 || shared.aborted.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                _ = notified => {}
                _ = shared.abort_notify.notified() => {}
            }
        }
        if shared.aborted.load(Ordering::SeqCst) {
            outcome = Closed::error(ERR_ABORTED, "Connection aborted".into());
            break;
        }
        let next = tokio::select! {
            next = stream.next() => next,
            _ = shared.abort_notify.notified() => {
                outcome = Closed::error(ERR_ABORTED, "Connection aborted".into());
                break;
            }
        };
        match next {
            Some(Ok(m @ (Message::Text(_) | Message::Binary(_)))) => {
                shared.credits.fetch_sub(charge(m.len()), Ordering::SeqCst);
                if frames.send(m).await.is_err() {
                    break;
                }
            }
            Some(Ok(Message::Pong(_))) => {
                // Pongs cost credits too, so a pong flood cannot queue
                // unbounded events in the isolate; Dart grants them back.
                shared.credits.fetch_sub(MESSAGE_OVERHEAD, Ordering::SeqCst);
                shared.post(&mut [
                    DartCObject::int(EV_PONG),
                    DartCObject::int(MESSAGE_OVERHEAD),
                ]);
            }
            Some(Ok(Message::Close(frame))) => {
                outcome = Closed {
                    code: frame.as_ref().map(|f| f.code.into()),
                    reason: frame.map(|f| f.reason.to_string()).unwrap_or_default(),
                    kind: ERR_NONE,
                    message: None,
                    status: None,
                    headers: None,
                };
                // tungstenite queued the close reply; one more poll flushes
                // it. Do not wait for the TCP close for long, and stop on
                // abort.
                tokio::select! {
                    _ = tokio::time::timeout(Duration::from_millis(250), stream.next()) => {}
                    _ = shared.abort_notify.notified() => {}
                }
                break;
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                let closed = map_error(e);
                if let Some(code) = closed.code {
                    owed_close = Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: closed.reason.clone().into(),
                    });
                }
                outcome = closed;
                break;
            }
            None => break,
        }
    }

    drop(frames);
    let aborted = shared.aborted.load(Ordering::SeqCst);
    shared.open.store(false, Ordering::SeqCst);
    if aborted {
        decoder.abort();
    }
    let _ = decoder.await;
    probe.abort();

    // Stop the writer. It may be parked in a write that never completes
    // (black-holed network): never wait for it without a bound.
    let writer_abort = writer.abort_handle();
    let sink = if aborted {
        drop(stop);
        writer_abort.abort();
        None
    } else {
        let _ = stop.send(());
        match tokio::time::timeout(Duration::from_secs(1), writer).await {
            Ok(Ok(sink)) => Some(sink),
            _ => {
                writer_abort.abort();
                None
            }
        }
    };
    if let Some(mut sink) = sink {
        if let Some(frame) = owed_close {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                sink.send(Message::Close(Some(frame))),
            )
            .await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    }
    outcome
}

enum Item {
    Plain(DartCObject),
    External(ExternalBytes),
}

/// Converts a batch of messages and posts it as one port message.
fn post_batch(shared: &Shared, json: bool, batch: &mut Vec<Message>) -> bool {
    let mut items: Vec<Item> = Vec::with_capacity(batch.len() * 2 + 2);
    items.push(Item::Plain(DartCObject::int(EV_BATCH)));
    items.push(Item::Plain(DartCObject::int(0)));
    let mut bytes = 0i64;
    for message in batch.drain(..) {
        bytes += charge(message.len());
        match message {
            Message::Text(text) => {
                if json && let Ok(t) = tape::build_tape(text.as_bytes().to_vec()) {
                    // The tape is larger than the text: charge the
                    // difference too (it is granted back with the batch).
                    let extra = t.bytes.saturating_sub(text.len()) as i64;
                    shared.credits.fetch_sub(extra, Ordering::SeqCst);
                    bytes += extra;
                    let (ptr, len) = (t.words.as_ptr() as *const u8, t.bytes);
                    items.push(Item::Plain(DartCObject::int(KIND_TAPE)));
                    items.push(Item::External(ExternalBytes::new(t.words, ptr, len, true)));
                    continue;
                }
                // Raw bytes; invalid JSON is reported by the Dart decoder.
                let (ptr, len) = (text.as_str().as_ptr(), text.len());
                items.push(Item::Plain(DartCObject::int(KIND_TEXT)));
                items.push(Item::External(ExternalBytes::new(text, ptr, len, false)));
            }
            Message::Binary(data) => {
                let (ptr, len) = (data.as_ptr(), data.len());
                items.push(Item::Plain(DartCObject::int(KIND_BINARY)));
                items.push(Item::External(ExternalBytes::new(data, ptr, len, false)));
            }
            _ => {}
        }
    }
    if let Item::Plain(o) = &mut items[1] {
        *o = DartCObject::int(bytes);
    }
    let mut pointers: Vec<*mut DartCObject> = items
        .iter_mut()
        .map(|item| match item {
            Item::Plain(o) => o as *mut DartCObject,
            Item::External(e) => &mut e.object as *mut DartCObject,
        })
        .collect();
    let mut array = DartCObject::array(&mut pointers);
    let posted = dart::post(shared.port, &mut array);
    // External buffers belong to Dart after a successful post.
    for item in items {
        if let Item::External(e) = item
            && !posted
        {
            e.release();
        }
    }
    if !posted {
        shared.abort();
    }
    posted
}

#[cfg(test)]
mod tests {
    use super::tape::build_tape;

    #[test]
    fn tape_layout() {
        let t = build_tape(br#"{"a":[1,true],"b":{}}"#.to_vec()).unwrap();
        let n = (t.words[0] & 0xFFFF_FFFF) as usize;
        let w = &t.words[1..=n];
        assert_eq!(w[0] >> 56, 0x7B);
        assert_eq!(w[1], 2);
        assert_eq!(w[4] >> 56, 0x5B);
    }

    #[test]
    fn config_defaults() {
        let c: super::Config = serde_json::from_str(r#"{"url":"ws://h"}"#).unwrap();
        assert_eq!(c.window, 8 << 20);
        assert!(!c.allow_insecure);
    }
}
