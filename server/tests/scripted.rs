//! Post-handshake scripted endpoints and `/log`.

mod common;

use std::time::{Duration, Instant};

use common::*;
use futures_util::SinkExt;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::{
    Message,
    protocol::{CloseFrame, frame::coding::CloseCode},
};

#[tokio::test]
async fn raw_presets_write_exact_bytes() {
    let s = server().await;
    let cases: &[(&str, &[u8])] = &[
        ("/raw?preset=rsv1&end=fin", &[0xC1, 2, b'h', b'i']),
        ("/raw?preset=close1b&end=fin", &[0x88, 1, 3]),
        (
            "/raw?preset=close-code-1005&end=fin",
            &[0x88, 2, 0x03, 0xED],
        ),
        ("/raw?preset=eof-in-header", &[0x82]),
        ("/raw?preset=eof-between-frags", &[0x01, 2, b'h', b'e']),
        ("/raw?hex=81026869&end=fin", &[0x81, 2, b'h', b'i']),
        (
            "/raw?preset=rsv1&hex=8100&end=fin",
            &[0xC1, 2, b'h', b'i', 0x81, 0],
        ),
    ];
    for (path, want) in cases {
        let (mut tcp, resp) = raw_upgrade(s.ports.ws, path).await;
        assert_eq!(resp.status, 101, "{path}");
        let (bytes, err) = read_all(&mut tcp, resp.body).await;
        assert_eq!(&bytes, want, "{path}");
        assert!(err.is_none(), "{path}: {err:?}");
    }
    let zero = raw_upgrade(s.ports.ws, "/raw?preset=zero-cont-x10000&end=fin").await;
    let (mut tcp, resp) = zero;
    let (bytes, _) = read_all(&mut tcp, resp.body).await;
    assert_eq!(split_frames(&bytes).len(), 10_002);

    assert_eq!(
        get_upgrade_status(s.ports.ws, "/raw?preset=nope").await,
        400
    );
    assert_eq!(get_upgrade_status(s.ports.ws, "/raw?hex=zz").await, 400);
}

async fn get_upgrade_status(port: u16, path: &str) -> u16 {
    http_raw(port, &upgrade_request(path, "")).await.status
}

#[tokio::test]
async fn raw_chunks_with_delay() {
    let s = server().await;
    let start = Instant::now();
    let (mut tcp, resp) =
        raw_upgrade(s.ports.ws, "/raw?hex=81026869&chunk=1&delay_ms=50&end=fin").await;
    let (bytes, _) = read_all(&mut tcp, resp.body).await;
    assert_eq!(bytes, [0x81, 2, b'h', b'i']);
    assert!(start.elapsed() >= Duration::from_millis(150));
}

#[tokio::test]
async fn raw_invalid_frames_fail_a_strict_client() {
    let s = server().await;
    for preset in [
        "utf8-bad",
        "rsv1",
        "opcode3",
        "masked",
        "ping126",
        "cont-first",
        "len64-msb",
    ] {
        let mut ws = open_ws(s.ports.ws, &format!("/raw?preset={preset}")).await;
        let got = next_msg(&mut ws).await;
        assert!(matches!(got, Some(Err(_))), "{preset}: {got:?}");
    }
}

#[tokio::test]
async fn raw_wait_ends_after_client_close() {
    let s = server().await;
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/raw?preset=close-code-3000&id=rw").await;
    let mut bytes = resp.body;
    while bytes.len() < 4 {
        bytes.extend(read_n(&mut tcp, 4 - bytes.len()).await);
    }
    assert_eq!(bytes, [0x88, 2, 0x0B, 0xB8]);
    tcp.write_all(&client_frame(true, 8, &[0x0B, 0xB8]))
        .await
        .unwrap();
    let (rest, err) = read_all(&mut tcp, Vec::new()).await;
    assert!(rest.is_empty() && err.is_none());
    drop(tcp);
    let log = log(s.ports.ws, "rw").await;
    assert_eq!(log["close"]["code"], 3000);
    assert_eq!(log["tcp"], "fin");
}

#[tokio::test]
async fn fragment_reassembles_for_a_client() {
    let s = server().await;
    let mut ws = open_ws(
        s.ports.ws,
        "/fragment?size=100&parts=7&split_utf8=1&ping_between=1",
    )
    .await;
    let Message::Text(t) = next_msg(&mut ws).await.unwrap().unwrap() else {
        panic!("expected text");
    };
    assert_eq!(t.as_str(), "€".repeat(34));
    roundtrip(&mut ws, Message::text("then echo")).await;

    let mut ws = open_ws(s.ports.ws, "/fragment?size=1000&parts=3&kind=binary").await;
    let expected: Vec<u8> = (0..1000).map(|i| i as u8).collect();
    assert_eq!(
        next_msg(&mut ws).await.unwrap().unwrap(),
        Message::binary(expected)
    );
}

#[tokio::test]
async fn fragment_wire_layout() {
    let s = server().await;
    let (mut tcp, resp) = raw_upgrade(
        s.ports.ws,
        "/fragment?size=30&parts=3&split_utf8=1&ping_between=1",
    )
    .await;
    let mut bytes = resp.body;
    // 3 data frames (2-byte headers, 30 bytes) + 2 pings ("between").
    let total = 3 * 2 + 30 + 2 * (2 + 7);
    if bytes.len() < total {
        bytes.extend(read_n(&mut tcp, total - bytes.len()).await);
    }
    let frames = split_frames(&bytes);
    let ops: Vec<(bool, u8)> = frames.iter().map(|f| (f.0, f.1)).collect();
    assert_eq!(
        ops,
        [(false, 1), (true, 9), (false, 0), (true, 9), (true, 0)]
    );
    // Boundaries fall inside code points, so the first fragment is not UTF-8.
    assert!(std::str::from_utf8(&frames[0].2).is_err());
}

#[tokio::test]
async fn log_records_client_frames_close_and_fin() {
    let s = server().await;
    let mut ws = open_ws(s.ports.ws, "/echo?id=t1").await;
    roundtrip(&mut ws, Message::text("hello")).await;
    ws.close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "bye".into(),
    }))
    .await
    .unwrap();
    while next_msg(&mut ws).await.is_some_and(|m| m.is_ok()) {}
    drop(ws);
    let log = log(s.ports.ws, "t1").await;
    let frames = log["frames"].as_array().unwrap();
    assert_eq!(frames[0]["op"], 1);
    assert_eq!(frames[0]["len"], 5);
    assert_eq!(frames[0]["masked"], true);
    assert_eq!(frames[0]["fin"], true);
    assert_eq!(frames[0]["rsv"], 0);
    assert_eq!(frames.last().unwrap()["op"], 8);
    assert_eq!(log["close"]["code"], 1000);
    assert_eq!(log["close"]["reason"], "bye");
    assert!(log["bytes"].as_u64().unwrap() >= 11 + 9);
    assert_eq!(log["tcp"], "fin");

    assert_eq!(get(s.ports.ws, "/log?id=unknown").await.status, 404);
}

#[tokio::test]
async fn log_works_over_tls_and_for_scripted_endpoints() {
    let s = server().await;
    let tls = tls(&s.ca_der, s.ports.wss, "localhost").await.unwrap();
    let url = format!("wss://localhost:{}/ignore-close?id=tl", s.ports.wss);
    let (mut ws, _) = tokio_tungstenite::client_async(url, tls).await.unwrap();
    ws.send(Message::binary(vec![1u8; 300])).await.unwrap();
    assert_eq!(
        next_msg(&mut ws).await.unwrap().unwrap(),
        Message::binary(vec![1u8; 300])
    );
    drop(ws);
    let log = log(s.ports.ws, "tl").await;
    assert_eq!(log["frames"][0]["op"], 2);
    assert_eq!(log["frames"][0]["len"], 300);
}

#[tokio::test]
async fn flood_then_close() {
    let s = server().await;
    let mut ws = open_ws(s.ports.ws, "/flood?count=1000&size=8&kind=binary").await;
    for _ in 0..1000 {
        assert_eq!(
            next_msg(&mut ws).await.unwrap().unwrap(),
            Message::binary(b"abcdefgh".to_vec())
        );
    }
    let Message::Close(Some(f)) = next_msg(&mut ws).await.unwrap().unwrap() else {
        panic!("expected close");
    };
    assert_eq!(f.code, CloseCode::Normal);

    let mut ws = open_ws(s.ports.ws, "/flood?count=3&then=stay").await;
    for _ in 0..3 {
        next_msg(&mut ws).await.unwrap().unwrap();
    }
    roundtrip(&mut ws, Message::text("still here")).await;
}

#[tokio::test]
async fn ping_reports_pong_result() {
    let s = server().await;
    // tungstenite answers pings automatically with the same payload.
    let mut ws = open_ws(s.ports.ws, "/ping?payload=cafe&count=2&interval_ms=20").await;
    for _ in 0..2 {
        assert_eq!(
            next_msg(&mut ws).await.unwrap().unwrap(),
            Message::text("pong-ok")
        );
    }
    // A wrong pong by hand.
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/ping?payload=01").await;
    let mut bytes = resp.body;
    if bytes.len() < 3 {
        bytes.extend(read_n(&mut tcp, 3 - bytes.len()).await);
    }
    assert_eq!(bytes, [0x89, 1, 1]);
    tcp.write_all(&client_frame(true, 0xA, &[2])).await.unwrap();
    let reply = read_n(&mut tcp, 2 + 13).await;
    assert_eq!(&reply[2..], b"pong-mismatch");
}

#[tokio::test]
async fn pong_sends_unsolicited_pongs() {
    let s = server().await;
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/pong?interval_ms=10").await;
    let mut bytes = resp.body;
    while bytes.len() < 6 {
        bytes.extend(read_n(&mut tcp, 2).await);
    }
    assert_eq!(&bytes[..6], [0x8A, 0, 0x8A, 0, 0x8A, 0]);
}

#[tokio::test]
async fn stall_sends_partial_frame() {
    let s = server().await;
    let (mut tcp, resp) = raw_upgrade(s.ports.ws, "/stall?size=1000&sent=10").await;
    let mut bytes = resp.body;
    if bytes.len() < 14 {
        bytes.extend(read_n(&mut tcp, 14 - bytes.len()).await);
    }
    assert_eq!(&bytes[..4], [0x82, 126, 0x03, 0xE8]);
    let more = tokio::time::timeout(Duration::from_millis(200), read_n(&mut tcp, 1)).await;
    assert!(more.is_err(), "server sent more than `sent` bytes");
}

#[tokio::test]
async fn slow_read_counts_bytes() {
    let s = server().await;
    let (mut tcp, _) = raw_upgrade(s.ports.ws, "/slow-read?bps=0&pause_ms=100&id=sr").await;
    tcp.write_all(&client_frame(true, 2, &[0u8; 1000]))
        .await
        .unwrap();
    tcp.shutdown().await.unwrap();
    let log = log(s.ports.ws, "sr").await;
    assert_eq!(log["bytes"], 1000 + 8);
    assert_eq!(log["tcp"], "fin");
}

#[tokio::test]
async fn silent_never_pongs() {
    let s = server().await;
    let mut ws = open_ws(s.ports.ws, "/silent").await;
    ws.send(Message::Ping(b"x".to_vec().into())).await.unwrap();
    let next = tokio::time::timeout(
        Duration::from_millis(300),
        futures_util::StreamExt::next(&mut ws),
    )
    .await;
    assert!(next.is_err(), "server sent something: {next:?}");
}
