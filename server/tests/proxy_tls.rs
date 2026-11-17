//! CONNECT proxy, TLS ports and IPv6.

mod common;

use common::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_tungstenite::{client_async, tungstenite::Message};

/// Sends a CONNECT and returns the stream plus the response head.
async fn connect_via(proxy: u16, target: &str, auth: Option<&str>) -> (TcpStream, String) {
    let mut tcp = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let auth = auth.map_or(String::new(), |a| format!("Proxy-Authorization: {a}\r\n"));
    let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth}\r\n");
    tcp.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        if tcp.read(&mut b).await.unwrap() == 0 {
            break;
        }
        head.push(b[0]);
    }
    (tcp, String::from_utf8(head).unwrap())
}

#[tokio::test]
async fn connect_tunnel_without_auth() {
    let s = server().await;
    let target = format!("127.0.0.1:{}", s.ports.ws);
    let (tcp, head) = connect_via(s.ports.proxy, &target, None).await;
    assert_eq!(head, "HTTP/1.1 200 Connection established\r\n\r\n");
    let (mut ws, _) = client_async(format!("ws://{target}/echo"), tcp)
        .await
        .unwrap();
    roundtrip(&mut ws, Message::text("through the tunnel")).await;

    let log: serde_json::Value =
        serde_json::from_slice(&get(s.ports.ws, "/proxy-log").await.body).unwrap();
    assert_eq!(log["tunnels"], serde_json::json!([target]));
}

#[tokio::test]
async fn connect_tunnel_with_auth() {
    let s = server().await;
    let target = format!("127.0.0.1:{}", s.ports.wss);

    let (mut tcp, head) = connect_via(s.ports.proxy_auth, &target, None).await;
    assert!(head.starts_with("HTTP/1.1 407 "), "{head}");
    assert!(
        head.contains("Proxy-Authenticate: Basic realm=\"ws\""),
        "{head}"
    );
    let mut rest = Vec::new();
    tcp.read_to_end(&mut rest).await.unwrap();

    let (_, head) = connect_via(s.ports.proxy_auth, &target, Some("Basic dGVzdDp3cm9uZw==")).await;
    assert!(head.starts_with("HTTP/1.1 407 "), "{head}");

    // TLS end to end through the tunnel.
    let (tcp, head) =
        connect_via(s.ports.proxy_auth, &target, Some("Basic dGVzdDpzZWNyZXQ=")).await;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tls = tls_connector(&s.ca_der).connect(name, tcp).await.unwrap();
    let (mut ws, _) = client_async(format!("wss://localhost:{}/echo", s.ports.wss), tls)
        .await
        .unwrap();
    roundtrip(&mut ws, Message::text("secret")).await;
}

#[tokio::test]
async fn connect_upstream_failure_is_502() {
    let s = server().await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (_, head) = connect_via(s.ports.proxy, &closed.to_string(), None).await;
    assert!(head.starts_with("HTTP/1.1 502 "), "{head}");
    let (_, head) = connect_via(s.ports.proxy, "", None).await;
    assert!(!head.starts_with("HTTP/1.1 200"), "{head}");
}

#[tokio::test]
async fn tls_with_generated_ca() {
    let s = server().await;
    for name in ["localhost", "127.0.0.1"] {
        let tls = tls(&s.ca_der, s.ports.wss, name).await.unwrap();
        let (mut ws, _) = client_async(format!("wss://{name}:{}/echo", s.ports.wss), tls)
            .await
            .unwrap();
        roundtrip(&mut ws, Message::text(name)).await;
    }
    // GET /ca.pem serves the same CA as the ready line.
    let resp = get(s.ports.ws, "/ca.pem").await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), s.ca_pem);
    assert!(s.ca_pem.starts_with("-----BEGIN CERTIFICATE-----"));
    // Fault endpoints work on the TLS port too.
    let mut tls = tls(&s.ca_der, s.ports.wss, "localhost").await.unwrap();
    tls.write_all(b"GET /status?code=418 HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tls.read_to_end(&mut buf).await;
    assert!(buf.starts_with(b"HTTP/1.1 418"));
}

#[tokio::test]
async fn badhost_cert_does_not_match() {
    let s = server().await;
    let err = tls(&s.ca_der, s.ports.wss_badhost, "localhost")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not valid for name"), "{err}");
    // It is signed by the same CA, so the right name verifies.
    let tls = tls(&s.ca_der, s.ports.wss_badhost, "wrong.invalid")
        .await
        .unwrap();
    let (mut ws, _) = client_async(
        format!("wss://wrong.invalid:{}/echo", s.ports.wss_badhost),
        tls,
    )
    .await
    .unwrap();
    roundtrip(&mut ws, Message::text("ok")).await;
}

#[tokio::test]
async fn ipv6_loopback_when_available() {
    let s = server().await;
    let Some(port) = s.ports.ws6 else {
        eprintln!("no IPv6 loopback; skipping");
        return;
    };
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://[::1]:{port}/echo"))
        .await
        .unwrap();
    roundtrip(&mut ws, Message::text("v6")).await;
}
