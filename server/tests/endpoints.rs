//! Endpoint behaviour, against an in-process server.

mod common;

use common::*;
use futures_util::SinkExt;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

#[tokio::test]
async fn echo_text_and_binary() {
    let s = server().await;
    let mut ws = open_ws(s.ports.ws, "/echo").await;
    roundtrip(&mut ws, Message::text("hello")).await;
    roundtrip(&mut ws, Message::binary(vec![0u8, 1, 2, 255])).await;
    roundtrip(&mut ws, Message::binary(vec![7u8; 200_000])).await;
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn echo_selects_first_offered_protocol() {
    let s = server().await;
    let url = format!("ws://127.0.0.1:{}/echo", s.ports.ws);
    let req = request_with(&url, &[("Sec-WebSocket-Protocol", "chat, superchat")]);
    let (_, resp) = connect_async(req).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "chat");

    // `protocol=` picks another offered one.
    let req = request_with(
        &format!("{url}?protocol=superchat"),
        &[("Sec-WebSocket-Protocol", "chat, superchat")],
    );
    let (_, resp) = connect_async(req).await.unwrap();
    assert_eq!(resp.headers()["sec-websocket-protocol"], "superchat");

    // Nothing offered, nothing selected.
    let (_, resp) = connect_async(url).await.unwrap();
    assert!(resp.headers().get("sec-websocket-protocol").is_none());
}

#[tokio::test]
async fn stream_sends_count_then_closes() {
    let s = server().await;
    let mut ws = open_ws(s.ports.ws, "/stream?format=binary&size=32&rate=0&count=50").await;
    for seq in 0..50u64 {
        let Message::Binary(b) = next_msg(&mut ws).await.unwrap().unwrap() else {
            panic!("expected binary");
        };
        assert_eq!(u64::from_le_bytes(b[..8].try_into().unwrap()), seq);
    }
    let Message::Close(Some(f)) = next_msg(&mut ws).await.unwrap().unwrap() else {
        panic!("expected close");
    };
    assert_eq!((f.code, f.reason.as_str()), (CloseCode::Normal, "done"));
}

#[tokio::test]
async fn status_answers_plain_http_and_fails_handshake() {
    let s = server().await;
    let resp = get(
        s.ports.ws,
        "/status?code=403&body=nope&header=Retry-After:5&header=X-A:b",
    )
    .await;
    assert_eq!(resp.status, 403);
    assert_eq!(resp.header("retry-after").as_deref(), Some("5"));
    assert_eq!(resp.header("x-a").as_deref(), Some("b"));
    assert_eq!(resp.header("connection").as_deref(), Some("close"));
    assert_eq!(resp.text(), "nope");

    let err = try_ws(s.ports.ws, "/status?code=503").await.unwrap_err();
    assert!(err.to_string().contains("503"), "{err}");
    let err = try_ws(s.ports.ws, "/status").await.unwrap_err();
    assert!(err.to_string().contains("200"), "{err}");
}

#[tokio::test]
async fn redirects() {
    let s = server().await;
    let resp = get(s.ports.ws, "/redirect?code=307&to=%2Fecho%3Fa%3D1").await;
    assert_eq!(resp.status, 307);
    assert_eq!(resp.header("location").as_deref(), Some("/echo?a=1"));

    let resp = get(s.ports.ws, "/redirect-loop?n=2&id=x").await;
    assert_eq!(
        resp.header("location").as_deref(),
        Some("/redirect-loop?n=1&id=x")
    );
    let resp = get(s.ports.ws, "/redirect-loop?n=0").await;
    assert_eq!(resp.header("location").as_deref(), Some("/echo"));
}

#[tokio::test]
async fn handshake_faults() {
    let s = server().await;
    let resp = http_raw(s.ports.ws, &upgrade_request("/junk", "")).await;
    assert_eq!(resp.head, "220 smtp.example ESMTP\r\n");
    let resp = http_raw(s.ports.ws, &upgrade_request("/eof-handshake?partial=1", "")).await;
    assert_eq!(resp.head, "HTTP/1.1 101 Swi");
    let resp = http_raw(s.ports.ws, &upgrade_request("/eof-handshake", "")).await;
    assert_eq!(resp.head, "");

    assert!(try_ws(s.ports.ws, "/bad-101?accept=wrong").await.is_err());
    assert!(try_ws(s.ports.ws, "/bad-101?omit=upgrade").await.is_err());
    assert!(
        try_ws(s.ports.ws, "/bad-101?protocol=unoffered")
            .await
            .is_err()
    );
    let mut ok = open_ws(s.ports.ws, "/bad-101").await;
    roundtrip(&mut ok, Message::text("still echo")).await;

    let (_, resp) = raw_upgrade(
        s.ports.ws,
        "/bad-101?accept=dup&extensions=permessage-deflate",
    )
    .await;
    assert_eq!(resp.head.matches("Sec-WebSocket-Accept").count(), 2);
    assert!(
        resp.head
            .contains("Sec-WebSocket-Extensions: permessage-deflate")
    );

    let (_, resp) = raw_upgrade(s.ports.ws, "/big-head?kb=20").await;
    assert!(resp.head.len() > 20 * 1024);

    // Oversized request head.
    let big = upgrade_request("/echo", &format!("X-Big: {}\r\n", "a".repeat(17 * 1024)));
    assert_eq!(http_raw(s.ports.ws, &big).await.status, 431);
    // WebSocket path without upgrade, unknown path.
    assert_eq!(get(s.ports.ws, "/echo").await.status, 426);
    assert_eq!(get(s.ports.ws, "/nope").await.status, 404);
}

#[tokio::test]
async fn headers_reports_request_then_echoes() {
    let s = server().await;
    let url = format!("ws://127.0.0.1:{}/headers?x=1", s.ports.ws);
    let req = request_with(&url, &[("X-Custom", "yes")]);
    let (mut ws, _) = connect_async(req).await.unwrap();
    let Message::Text(json) = next_msg(&mut ws).await.unwrap().unwrap() else {
        panic!("expected text");
    };
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["method"], "GET");
    assert_eq!(v["target"], "/headers?x=1");
    assert_eq!(v["version"], "HTTP/1.1");
    assert!(v["peer"].as_str().unwrap().starts_with("127.0.0.1:"));
    let headers = v["headers"].as_array().unwrap();
    assert!(
        headers
            .iter()
            .any(|h| h[0].as_str().unwrap().eq_ignore_ascii_case("x-custom") && h[1] == "yes")
    );
    roundtrip(&mut ws, Message::text("after")).await;
}

#[tokio::test]
async fn delay_then_routes() {
    let s = server().await;
    let start = std::time::Instant::now();
    let mut ws = open_ws(s.ports.ws, "/delay?ms=200&then=/echo").await;
    assert!(start.elapsed().as_millis() >= 200);
    roundtrip(&mut ws, Message::text("late")).await;
    let resp = get(s.ports.ws, "/delay?ms=10&then=%2Fstatus%3Fcode%3D418").await;
    assert_eq!(resp.status, 418);
}

#[tokio::test]
async fn close_variants() {
    let s = server().await;
    let mut ws = open_ws(s.ports.ws, "/close?code=4000&reason=bye").await;
    let Message::Close(Some(f)) = next_msg(&mut ws).await.unwrap().unwrap() else {
        panic!("expected close frame");
    };
    assert_eq!((u16::from(f.code), f.reason.as_str()), (4000, "bye"));

    let mut ws = open_ws(s.ports.ws, "/close-empty").await;
    assert_eq!(
        next_msg(&mut ws).await.unwrap().unwrap(),
        Message::Close(None)
    );

    // Echo two messages first.
    let mut ws = open_ws(s.ports.ws, "/close?after_msgs=2&code=1001").await;
    roundtrip(&mut ws, Message::text("1")).await;
    roundtrip(&mut ws, Message::text("2")).await;
    let Message::Close(Some(f)) = next_msg(&mut ws).await.unwrap().unwrap() else {
        panic!("expected close frame");
    };
    assert_eq!(f.code, CloseCode::Away);

    // then=rst: close frame, then a reset.
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/close?code=1000&then=rst").await;
    let (bytes, err) = read_all(&mut tcp, resp.body).await;
    assert_eq!(bytes, [0x88, 2, 0x03, 0xE8]);
    assert_eq!(
        err.map(|e| e.kind()),
        Some(std::io::ErrorKind::ConnectionReset)
    );

    // then=fin: close frame, then EOF without waiting for ours.
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/close?code=1000&then=fin").await;
    let (bytes, err) = read_all(&mut tcp, resp.body).await;
    assert_eq!(bytes, [0x88, 2, 0x03, 0xE8]);
    assert!(err.is_none());
}

#[tokio::test]
async fn drop_and_ignore_close() {
    let s = server().await;
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/drop?mode=fin").await;
    let (bytes, err) = read_all(&mut tcp, resp.body).await;
    assert!(bytes.is_empty() && err.is_none(), "{bytes:?} {err:?}");
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/drop?mode=rst&after_ms=50").await;
    let (bytes, err) = read_all(&mut tcp, resp.body).await;
    assert!(bytes.is_empty());
    assert_eq!(
        err.map(|e| e.kind()),
        Some(std::io::ErrorKind::ConnectionReset)
    );

    // The close is never answered: no close frame within a short wait.
    let mut ws = open_ws(s.ports.ws, "/ignore-close?id=ign").await;
    roundtrip(&mut ws, Message::text("x")).await;
    ws.send(Message::Close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "".into(),
    })))
    .await
    .unwrap();
    let next = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        futures_util::StreamExt::next(&mut ws),
    )
    .await;
    assert!(next.is_err(), "server answered the close: {next:?}");
}
