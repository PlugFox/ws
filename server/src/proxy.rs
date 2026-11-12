//! HTTP CONNECT proxy for proxy tests. Two listeners: one open, one that
//! requires `Proxy-Authorization: Basic base64("test:secret")`. Every
//! CONNECT target is recorded (see `/proxy-log`), authorized or not.

use std::{sync::Arc, time::Duration};

use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

use crate::{
    Shared,
    http::{self, HeadError},
};

/// `base64("test:secret")`.
const CREDENTIALS: &str = "dGVzdDpzZWNyZXQ=";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Accepts proxy clients forever.
pub async fn accept_loop(listener: TcpListener, auth: bool, shared: Arc<Shared>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            continue;
        };
        let shared = shared.clone();
        tokio::spawn(async move {
            let _ = serve(tcp, auth, &shared).await;
        });
    }
}

async fn serve(mut client: TcpStream, auth: bool, shared: &Shared) -> std::io::Result<()> {
    client.set_nodelay(true)?;
    let (req, rest) = match http::read_head(&mut client).await {
        Ok(head) => head,
        Err(HeadError::Io(e)) => return Err(e),
        Err(_) => return http::respond(&mut client, 400, &[], b"").await,
    };
    if !req.method.eq_ignore_ascii_case("CONNECT") {
        return http::respond(&mut client, 405, &[("Allow", "CONNECT")], b"CONNECT only").await;
    }
    shared.logs.tunnel(&req.target);
    if auth && !authorized(req.header("proxy-authorization")) {
        let challenge = [("Proxy-Authenticate", "Basic realm=\"ws\"")];
        return http::respond(&mut client, 407, &challenge, b"").await;
    }
    let mut upstream =
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&req.target)).await {
            Ok(Ok(s)) => s,
            _ => return http::respond(&mut client, 502, &[], b"upstream connect failed").await,
        };
    upstream.set_nodelay(true)?;
    client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
    upstream.write_all(&rest).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Checks `Basic <credentials>` (scheme case-insensitive).
fn authorized(header: Option<&str>) -> bool {
    header
        .and_then(|h| h.trim().split_once(' '))
        .is_some_and(|(scheme, value)| {
            scheme.eq_ignore_ascii_case("basic") && value.trim() == CREDENTIALS
        })
}
