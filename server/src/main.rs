//! Standalone benchmark server; see the library docs for the endpoints.
//!
//! Usage: `ws_bench_server [--host 127.0.0.1] [--port 9090] [--tls-port 9443]`

use std::collections::HashMap;

use ws_bench_server::{BoxError, listen};

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = parse_args();
    let host = args
        .get("host")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = args.get("port").map_or(Ok(9090), |v| v.parse())?;
    let tls_port: u16 = args.get("tls-port").map_or(Ok(9443), |v| v.parse())?;

    listen(&host, port, tls_port).await?;
    eprintln!("ws  listening on ws://{host}:{port}");
    eprintln!("wss listening on wss://{host}:{tls_port} (self-signed)");

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
