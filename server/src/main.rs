//! Standalone server; see the library docs and README for the endpoints.
//!
//! Usage:
//! `ws_bench_server [--host 127.0.0.1] [--port 9090] [--tls-port 9443]
//!  [--badhost-port 0] [--expired-port 0]
//!  [--proxy-port 0] [--proxy-auth-port 0] [--no-ipv6]
//!  [--tls-cert chain.pem --tls-key key.pem] [--ready-json] [--verbose]`
//!
//! With `--ready-json` the server prints exactly one JSON line to stdout
//! once every listener is bound, and exits when stdin reaches EOF (so a
//! killed test runner never leaves it behind). Otherwise it prints a
//! human-readable summary to stderr. Either way it stops on ctrl-c/SIGTERM.

use std::{collections::HashMap, io::Write as _, sync::atomic::Ordering};

use tokio::io::AsyncReadExt;
use ws_bench_server::{BoxError, Options, QUIET, describe, start};

/// Flags that take no value.
const SWITCHES: &[&str] = &["ready-json", "no-ipv6", "verbose"];

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = parse_args();
    let port = |name: &str, default: u16| args.get(name).map_or(Ok(default), |v| v.parse::<u16>());
    let opts = Options {
        host: args
            .get("host")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".into()),
        port: port("port", 9090)?,
        tls_port: port("tls-port", 9443)?,
        badhost_port: port("badhost-port", 0)?,
        expired_port: port("expired-port", 0)?,
        proxy_port: port("proxy-port", 0)?,
        proxy_auth_port: port("proxy-auth-port", 0)?,
        ipv6: !args.contains_key("no-ipv6"),
        tls_cert: match (args.get("tls-cert"), args.get("tls-key")) {
            (Some(cert), Some(key)) => Some((cert.clone(), key.clone())),
            (None, None) => None,
            _ => return Err("--tls-cert and --tls-key go together".into()),
        },
    };
    let ready_json = args.contains_key("ready-json");
    QUIET.store(
        ready_json && !args.contains_key("verbose"),
        Ordering::Relaxed,
    );

    let server = start(&opts).await?;
    if ready_json {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{}", server.ready_json())?;
        out.flush()?;
    } else {
        eprintln!("{}", describe(&opts.host, &server.ports));
    }

    tokio::select! {
        _ = shutdown_signal() => {}
        _ = stdin_eof(), if ready_json => {}
    }
    // Connection tasks may still hold sockets; do not wait for them.
    std::process::exit(0);
}

/// Resolves on ctrl-c, or SIGTERM on Unix.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Resolves when stdin is closed (the parent process went away).
async fn stdin_eof() {
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 256];
    while let Ok(n) = stdin.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

/// `--key value`, `--key=value` and bare switches (see [`SWITCHES`]).
fn parse_args() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let Some(key) = arg.strip_prefix("--") else {
            continue;
        };
        if let Some((k, v)) = key.split_once('=') {
            map.insert(k.to_owned(), v.to_owned());
        } else if SWITCHES.contains(&key) {
            map.insert(key.to_owned(), String::new());
        } else if let Some(v) = it.next() {
            map.insert(key.to_owned(), v);
        }
    }
    map
}
