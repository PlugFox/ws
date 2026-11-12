//! WebSocket server for benchmarks, tests and examples of the `ws` package.
//!
//! It runs its own HTTP front end: the request head is read and routed here
//! (see [`endpoints`]), HTTP-level faults are answered directly, and the
//! `101` is written by hand. Well-behaved endpoints (`/echo`, `/stream`,
//! ...) then hand the socket to tungstenite; scripted fault endpoints write
//! raw frames. See `README.md` for the endpoint list.
//!
//! Listeners started by [`start`]:
//!
//! * `ws`: plain endpoints, plus `GET /ca.pem`, `/log?id=`, `/proxy-log`.
//! * `wss`: TLS, certificate for `localhost`, `127.0.0.1`, `::1`.
//! * `wss_badhost`: TLS, certificate for `wrong.invalid` only.
//! * `wss_expired`: TLS, a certificate that expired in 2021.
//! * `ws6`: plain endpoints on `[::1]`, when IPv6 is available.
//! * `proxy` / `proxy_auth`: HTTP CONNECT proxies.
//!
//! Both certificates are signed by a CA generated at startup
//! ([`Server::ca_pem`]). The binary (`src/main.rs`) runs it standalone;
//! [`start_embedded`] runs it inside another process (the mobile benchmark
//! app) on its own runtime.

pub mod conn;
pub mod endpoints;
pub mod frame;
pub mod handshake;
pub mod http;
pub mod log;
pub mod proxy;
pub mod tls;

use std::{
    cell::Cell,
    fmt::Write as _,
    net::{IpAddr, Ipv6Addr},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use crate::{conn::Conn, log::Logs, tls::Pki};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// When set, per-connection errors are not printed to stderr (fault tests
/// produce many expected ones).
pub static QUIET: AtomicBool = AtomicBool::new(false);

/// State shared by every listener of one server.
pub struct Shared {
    pub logs: Logs,
    pub ca_pem: String,
}

/// Where to listen. Port `0` picks an ephemeral port.
#[derive(Clone, Debug)]
pub struct Options {
    pub host: String,
    pub port: u16,
    pub tls_port: u16,
    pub badhost_port: u16,
    pub expired_port: u16,
    pub proxy_port: u16,
    pub proxy_auth_port: u16,
    /// Also listen on `[::1]` (same port number as `port` unless `port` is 0).
    pub ipv6: bool,
    /// PEM files (chain, private key) the `wss` port serves instead of the
    /// generated certificate: a CA installed on a test device. `wss_spki`
    /// is then empty.
    pub tls_cert: Option<(String, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            host: "127.0.0.1".into(),
            port: 0,
            tls_port: 0,
            badhost_port: 0,
            expired_port: 0,
            proxy_port: 0,
            proxy_auth_port: 0,
            ipv6: true,
            tls_cert: None,
        }
    }
}

/// Actual bound ports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ports {
    pub ws: u16,
    pub wss: u16,
    pub wss_badhost: u16,
    pub wss_expired: u16,
    pub ws6: Option<u16>,
    pub proxy: u16,
    pub proxy_auth: u16,
}

/// A running server (its tasks live on the runtime that called [`start`]).
pub struct Server {
    pub ports: Ports,
    /// PEM of the CA that signed both TLS certificates.
    pub ca_pem: String,
    /// DER of the same CA certificate.
    pub ca_der: Vec<u8>,
    /// SubjectPublicKeyInfo (DER) of the `wss` certificate.
    pub wss_spki: Vec<u8>,
}

impl Server {
    /// The single-line JSON the binary prints with `--ready-json`.
    pub fn ready_json(&self) -> String {
        let p = &self.ports;
        let ws6 = p.ws6.map_or("null".into(), |v| v.to_string());
        let mut out = format!(
            "{{\"ws\":{},\"wss\":{},\"wss_badhost\":{},\"wss_expired\":{},\"ws6\":{ws6},\"proxy\":{},\"proxy_auth\":{},\"ca_pem\":",
            p.ws, p.wss, p.wss_badhost, p.wss_expired, p.proxy, p.proxy_auth
        );
        http::json_str(&mut out, &self.ca_pem);
        out.push_str(",\"wss_spki\":\"");
        for b in &self.wss_spki {
            out.push_str(&format!("{b:02x}"));
        }
        out.push_str("\"}");
        out
    }
}

/// Binds every listener and spawns the accept loops on the current runtime.
pub async fn start(opts: &Options) -> Result<Server, BoxError> {
    let pki = Pki::generate()?;
    let shared = Arc::new(Shared {
        logs: Logs::default(),
        ca_pem: pki.ca_pem.clone(),
    });
    let host = opts.host.as_str();
    let plain = TcpListener::bind((host, opts.port)).await?;
    let secure = TcpListener::bind((host, opts.tls_port)).await?;
    let badhost = TcpListener::bind((host, opts.badhost_port)).await?;
    let expired = TcpListener::bind((host, opts.expired_port)).await?;
    let proxy = TcpListener::bind((host, opts.proxy_port)).await?;
    let proxy_auth = TcpListener::bind((host, opts.proxy_auth_port)).await?;
    let ws = plain.local_addr()?.port();
    let ipv6 = if opts.ipv6 {
        bind_ipv6(host, ws, opts.port).await
    } else {
        None
    };
    let ports = Ports {
        ws,
        wss: secure.local_addr()?.port(),
        wss_badhost: badhost.local_addr()?.port(),
        wss_expired: expired.local_addr()?.port(),
        ws6: ipv6.as_ref().map(|(_, p)| *p),
        proxy: proxy.local_addr()?.port(),
        proxy_auth: proxy_auth.local_addr()?.port(),
    };
    tokio::spawn(accept_loop(plain, None, shared.clone()));
    let (good, wss_spki) = match &opts.tls_cert {
        Some((cert, key)) => (tls::from_files(cert, key)?, Vec::new()),
        None => (pki.good, pki.good_spki),
    };
    tokio::spawn(accept_loop(secure, Some(good), shared.clone()));
    tokio::spawn(accept_loop(badhost, Some(pki.badhost), shared.clone()));
    tokio::spawn(accept_loop(expired, Some(pki.expired), shared.clone()));
    if let Some((Some(l), _)) = ipv6 {
        tokio::spawn(accept_loop(l, None, shared.clone()));
    }
    tokio::spawn(proxy::accept_loop(proxy, false, shared.clone()));
    tokio::spawn(proxy::accept_loop(proxy_auth, true, shared));
    Ok(Server {
        ports,
        ca_pem: pki.ca_pem,
        ca_der: pki.ca_der,
        wss_spki,
    })
}

/// Binds `[::1]` for the plain endpoints. Returns the listener (none when
/// `host` already is IPv6 loopback, so the main listener covers it) and
/// the port, or `None` when IPv6 is unavailable.
async fn bind_ipv6(host: &str, ws: u16, requested: u16) -> Option<(Option<TcpListener>, u16)> {
    if host.parse::<IpAddr>().ok() == Some(IpAddr::V6(Ipv6Addr::LOCALHOST)) {
        return Some((None, ws));
    }
    let port = if requested == 0 { 0 } else { ws };
    let listener = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await.ok()?;
    let port = listener.local_addr().ok()?.port();
    Some((Some(listener), port))
}

/// Binds the listeners and spawns their accept loops on the current
/// runtime (kept for the benchmarks; see [`start`]).
pub async fn listen(host: &str, port: u16, tls_port: u16) -> Result<(), BoxError> {
    let opts = Options {
        host: host.into(),
        port,
        tls_port,
        ..Options::default()
    };
    start(&opts).await.map(|_| ())
}

async fn accept_loop(listener: TcpListener, tls: Option<TlsAcceptor>, shared: Arc<Shared>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            continue;
        };
        let tls = tls.clone();
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(tcp, tls, shared).await
                && !QUIET.load(Ordering::Relaxed)
            {
                eprintln!("connection error: {e}");
            }
        });
    }
}

/// Wraps an accepted socket (TLS if needed) and hands it to the router.
async fn serve(
    tcp: TcpStream,
    tls: Option<TlsAcceptor>,
    shared: Arc<Shared>,
) -> Result<(), BoxError> {
    tcp.set_nodelay(true)?;
    let peer = tcp.peer_addr().ok();
    let (tcp, socket) = split_socket(tcp)?;
    let conn = match tls {
        Some(acceptor) => Conn::new(
            Box::new(acceptor.accept(tcp).await?),
            Some(socket),
            peer,
            true,
        ),
        None => Conn::new(Box::new(tcp), Some(socket), peer, false),
    };
    endpoints::serve(conn, shared).await
}

/// Duplicates the socket handle so endpoints can set `SO_LINGER` later,
/// even after the stream is wrapped in TLS.
fn split_socket(tcp: TcpStream) -> std::io::Result<(TcpStream, socket2::Socket)> {
    let std = tcp.into_std()?;
    let dup = socket2::Socket::from(std.try_clone()?);
    Ok((TcpStream::from_std(std)?, dup))
}

// --- Embedded mode (mobile benchmark app) ---------------------------------

static EMBEDDED: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static EMBEDDED_CPU_NANOS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static LAST_CPU_NANOS: Cell<u64> = const { Cell::new(0) };
}

/// Thread CPU time; 0 where unsupported (Windows: the embedded server
/// runs only inside the mobile benchmark app).
#[cfg(not(unix))]
fn thread_cpu_nanos() -> u64 {
    0
}

#[cfg(unix)]
fn thread_cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
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
pub fn start_embedded(
    host: &str,
    port: u16,
    tls_port: u16,
    threads: usize,
) -> Result<(), BoxError> {
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

/// Human-readable listener summary (the binary prints it to stderr).
pub fn describe(host: &str, ports: &Ports) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "ws  listening on ws://{host}:{}", ports.ws);
    let _ = writeln!(
        out,
        "wss listening on wss://{host}:{} (test CA, GET /ca.pem)",
        ports.wss
    );
    let _ = writeln!(
        out,
        "wss (cert for wrong.invalid) on wss://{host}:{}",
        ports.wss_badhost
    );
    let _ = writeln!(
        out,
        "wss (expired cert) on wss://{host}:{}",
        ports.wss_expired
    );
    if let Some(p) = ports.ws6 {
        let _ = writeln!(out, "ws6 listening on ws://[::1]:{p}");
    }
    let _ = writeln!(out, "proxy (CONNECT) on {host}:{}", ports.proxy);
    let _ = write!(
        out,
        "proxy (CONNECT, Basic test:secret) on {host}:{}",
        ports.proxy_auth
    );
    out
}
