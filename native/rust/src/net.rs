//! TCP connection setup: Happy Eyeballs (RFC 8305) over every resolved
//! address, and an optional HTTP CONNECT proxy. Mirrors what `dart:io`
//! `Socket.startConnect` does (DECISIONS.md D34).

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, lookup_host};

/// Delay before the next address is tried while earlier attempts are still
/// pending (RFC 8305 §5 recommends 250 ms; `dart:io` uses the same).
pub const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// HTTP CONNECT proxy from the Dart options.
#[derive(Deserialize, Debug, Clone)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    /// Complete `Proxy-Authorization` value (e.g. `Basic …`), if any.
    #[serde(default)]
    pub authorization: Option<String>,
}

/// Opens a TCP connection to `host:port`, directly or through `proxy`.
/// With `keepalive`, TCP keepalive starts after that much idle time
/// (see [`keep_alive`]).
pub async fn connect(
    host: &str,
    port: u16,
    proxy: Option<&Proxy>,
    keepalive: Option<Duration>,
) -> io::Result<TcpStream> {
    let mut tcp = match proxy {
        None => connect_direct(host, port).await?,
        Some(proxy) => {
            let mut tcp = connect_direct(&proxy.host, proxy.port).await?;
            tunnel(&mut tcp, host, port, proxy.authorization.as_deref()).await?;
            tcp
        }
    };
    if let Some(idle) = keepalive {
        // Best effort: a second line behind the heartbeat, never a reason
        // to fail the connection.
        let _ = keep_alive(&mut tcp, idle);
    }
    Ok(tcp)
}

/// Kernel-level liveness that also works while the isolate is suspended or
/// blocked (DECISIONS.md D41): probes after `idle`, then every `idle / 3`
/// (at least 1 s), 3 probes. On Linux and Android `TCP_USER_TIMEOUT` bounds
/// unacknowledged data by the same total, so a black-holed write is
/// detected too. Windows ignores the probe count (10 probes).
pub fn keep_alive(tcp: &mut TcpStream, idle: Duration) -> io::Result<()> {
    let interval = (idle / 3).max(Duration::from_secs(1));
    let params = socket2::TcpKeepalive::new()
        .with_time(idle)
        .with_interval(interval);
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))]
    let params = params.with_retries(3);
    let socket = socket2::SockRef::from(&*tcp);
    socket.set_tcp_keepalive(&params)?;
    #[cfg(any(target_os = "android", target_os = "linux"))]
    socket.set_tcp_user_timeout(Some(idle + interval * 3))?;
    Ok(())
}

async fn connect_direct(host: &str, port: u16) -> io::Result<TcpStream> {
    // getaddrinfo on a blocking thread: on iOS it synthesizes NAT64
    // addresses for IPv4-only names.
    let addrs: Vec<SocketAddr> = lookup_host((host, port)).await?.collect();
    let stream = race(interleave(addrs), ATTEMPT_DELAY, TcpStream::connect).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Orders addresses for Happy Eyeballs: alternate families, starting with
/// the family of the first (preferred) address, keeping the resolver's order
/// within a family.
pub fn interleave(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return addrs;
    };
    let first_v6 = first.is_ipv6();
    let (mut primary, mut secondary): (Vec<_>, Vec<_>) =
        addrs.into_iter().partition(|a| a.is_ipv6() == first_v6);
    let mut out = Vec::with_capacity(primary.len() + secondary.len());
    primary.reverse();
    secondary.reverse();
    loop {
        match (primary.pop(), secondary.pop()) {
            (None, None) => return out,
            (a, b) => out.extend(a.into_iter().chain(b)),
        }
    }
}

/// Connects to the first address that answers. A new attempt starts when the
/// previous one fails or after `delay`, whichever comes first; the first
/// success wins and the other attempts are dropped (closing their sockets).
/// Returns the last error if every attempt fails.
pub async fn race<A, T, F, Fut>(addrs: Vec<A>, delay: Duration, mut connect: F) -> io::Result<T>
where
    F: FnMut(A) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let mut addrs = addrs.into_iter();
    let mut attempts = FuturesUnordered::new();
    match addrs.next() {
        Some(addr) => attempts.push(connect(addr)),
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "host resolved to no addresses",
            ));
        }
    }
    loop {
        let more = addrs.len() > 0;
        tokio::select! {
            biased;
            result = attempts.next(), if !attempts.is_empty() => match result {
                Some(Ok(stream)) => return Ok(stream),
                Some(Err(e)) => {
                    if let Some(addr) = addrs.next() {
                        attempts.push(connect(addr));
                    } else if attempts.is_empty() {
                        return Err(e);
                    }
                }
                None => unreachable!("guarded by is_empty"),
            },
            () = tokio::time::sleep(delay), if more => {
                attempts.push(connect(addrs.next().expect("more")));
            }
        }
    }
}

/// Largest proxy response head we accept.
const MAX_PROXY_RESPONSE: usize = 16 << 10;

/// Sends `CONNECT host:port` and reads the response head. Anything but a 2xx
/// is an error.
pub async fn tunnel<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    authorization: Option<&str>,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(value) = authorization {
        request.push_str("Proxy-Authorization: ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    // Read byte by byte up to the blank line: whatever follows belongs to
    // the tunnelled connection and must stay in the socket.
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_PROXY_RESPONSE {
            return Err(proxy_error("proxy response head is too long".into()));
        }
        if stream.read(&mut byte).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy closed the connection",
            ));
        }
        head.push(byte[0]);
    }
    let status_line = head
        .split(|&b| b == b'\r')
        .next()
        .map(String::from_utf8_lossy)
        .unwrap_or_default()
        .into_owned();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(200..=299) => Ok(()),
        _ => Err(proxy_error(format!("proxy refused CONNECT: {status_line}"))),
    }
}

fn proxy_error(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[tokio::test]
    async fn keepalive_is_set() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let tcp = connect("127.0.0.1", port, None, Some(Duration::from_secs(30)))
            .await
            .unwrap();
        let socket = socket2::SockRef::from(&tcp);
        assert!(socket.keepalive().unwrap());
        #[cfg(not(windows))]
        {
            assert_eq!(
                socket.tcp_keepalive_time().unwrap(),
                Duration::from_secs(30)
            );
            assert_eq!(
                socket.tcp_keepalive_interval().unwrap(),
                Duration::from_secs(10)
            );
            assert_eq!(socket.tcp_keepalive_retries().unwrap(), 3);
        }
        #[cfg(any(target_os = "android", target_os = "linux"))]
        assert_eq!(
            socket.tcp_user_timeout().unwrap(),
            Some(Duration::from_secs(60))
        );
        let plain = connect("127.0.0.1", port, None, None).await.unwrap();
        assert!(!socket2::SockRef::from(&plain).keepalive().unwrap());
    }
    use std::sync::{Arc, Mutex};
    use tokio::time::Instant;

    fn v4(n: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)), 80)
    }

    fn v6(n: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, n)), 80)
    }

    #[test]
    fn interleave_alternates_starting_with_first_family() {
        let got = interleave(vec![v6(1), v6(2), v6(3), v4(1), v4(2)]);
        assert_eq!(got, vec![v6(1), v4(1), v6(2), v4(2), v6(3)]);
        let got = interleave(vec![v4(1), v6(1), v4(2)]);
        assert_eq!(got, vec![v4(1), v6(1), v4(2)]);
        assert!(interleave(vec![]).is_empty());
    }

    /// Fake attempt: (outcome after delay ms, succeed?).
    type Plan = Vec<(u64, bool)>;

    async fn run(plan: Plan) -> (io::Result<usize>, Vec<(usize, u128)>) {
        let started = Arc::new(Mutex::new(Vec::new()));
        let t0 = Instant::now();
        let log = started.clone();
        let result = race((0..plan.len()).collect(), ATTEMPT_DELAY, move |i: usize| {
            log.lock().unwrap().push((i, t0.elapsed().as_millis()));
            let (ms, ok) = plan[i];
            async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                if ok {
                    Ok(i)
                } else {
                    Err(io::Error::other(format!("attempt {i} failed")))
                }
            }
        })
        .await;
        let started = started.lock().unwrap().clone();
        (result, started)
    }

    #[tokio::test(start_paused = true)]
    async fn first_address_answers_quickly() {
        let (r, started) = run(vec![(10, true), (10, true)]).await;
        assert_eq!(r.unwrap(), 0);
        assert_eq!(started, vec![(0, 0)], "second attempt never started");
    }

    #[tokio::test(start_paused = true)]
    async fn black_holed_first_address_loses_to_second_after_delay() {
        // Address 0 hangs (e.g. IPv6 route missing), 1 answers in 20 ms.
        let (r, started) = run(vec![(75_000, true), (20, true)]).await;
        assert_eq!(r.unwrap(), 1);
        assert_eq!(started, vec![(0, 0), (1, 250)]);
    }

    #[tokio::test(start_paused = true)]
    async fn failure_starts_next_attempt_immediately() {
        let (r, started) = run(vec![(5, false), (5, false), (5, true)]).await;
        assert_eq!(r.unwrap(), 2);
        assert_eq!(started, vec![(0, 0), (1, 5), (2, 10)]);
    }

    #[tokio::test(start_paused = true)]
    async fn all_failing_returns_last_error() {
        let (r, started) = run(vec![(300, false), (100, false)]).await;
        let e = r.unwrap_err();
        // 0 fails at 300 ms, 1 (started at 250) at 350 ms: the last one is reported.
        assert_eq!(e.to_string(), "attempt 1 failed");
        assert_eq!(started.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn earlier_attempt_can_still_win() {
        // 0 answers at 400 ms; 1 (started at 250) would answer at 750.
        let (r, _) = run(vec![(400, true), (500, true)]).await;
        assert_eq!(r.unwrap(), 0);
    }

    #[tokio::test]
    async fn no_addresses() {
        let r = race(Vec::<usize>::new(), ATTEMPT_DELAY, |_| async {
            Ok::<_, io::Error>(())
        })
        .await;
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    async fn tunnel_with(response: &'static [u8]) -> (io::Result<()>, String, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut buf = vec![0u8; 512];
            let n = server.read(&mut buf).await.unwrap();
            server.write_all(response).await.unwrap();
            String::from_utf8(buf[..n].to_vec()).unwrap()
        });
        let r = tunnel(&mut client, "example.com", 443, Some("Basic dTpw")).await;
        let request = server.await.unwrap();
        let mut rest = Vec::new();
        if r.is_ok() {
            let _ = tokio::time::timeout(Duration::from_millis(50), client.read_to_end(&mut rest))
                .await;
        }
        (r, request, rest)
    }

    #[tokio::test]
    async fn tunnel_ok_leaves_following_bytes() {
        let (r, request, rest) =
            tunnel_with(b"HTTP/1.1 200 Connection established\r\nX: y\r\n\r\nHELLO").await;
        r.unwrap();
        assert_eq!(
            request,
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\
             Proxy-Authorization: Basic dTpw\r\n\r\n"
        );
        assert_eq!(rest, b"HELLO");
    }

    #[tokio::test]
    async fn tunnel_refused() {
        let (r, _, _) = tunnel_with(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;
        let e = r.unwrap_err();
        assert!(e.to_string().contains("407"), "{e}");
    }

    #[tokio::test]
    async fn tunnel_eof() {
        let (r, _, _) = tunnel_with(b"HTTP/1.1 200").await;
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn tunnel_ipv6_authority() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let h = tokio::spawn(async move {
            let mut buf = vec![0u8; 512];
            let n = server.read(&mut buf).await.unwrap();
            server.write_all(b"HTTP/1.0 200 OK\r\n\r\n").await.unwrap();
            String::from_utf8(buf[..n].to_vec()).unwrap()
        });
        tunnel(&mut client, "::1", 8080, None).await.unwrap();
        assert!(
            h.await
                .unwrap()
                .starts_with("CONNECT [::1]:8080 HTTP/1.1\r\n")
        );
    }
}
