//! The binary's `--ready-json` contract, as the Dart test harness uses it.

mod common;

use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use common::*;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn ready_line_and_exit_on_stdin_eof() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ws_bench_server"))
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--tls-port",
            "0",
            "--ready-json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();

    let keys = [
        "ws",
        "wss",
        "wss_badhost",
        "wss_expired",
        "ws6",
        "proxy",
        "proxy_auth",
        "ca_pem",
    ];
    let obj = ready.as_object().unwrap();
    assert_eq!(obj.len(), keys.len(), "{line}");
    for k in keys {
        assert!(obj.contains_key(k), "missing {k}: {line}");
    }
    let port = |k: &str| ready[k].as_u64().unwrap() as u16;
    for k in [
        "ws",
        "wss",
        "wss_badhost",
        "wss_expired",
        "proxy",
        "proxy_auth",
    ] {
        assert_ne!(port(k), 0, "{k}");
    }
    assert!(ready["ws6"].is_null() || ready["ws6"].as_u64().is_some());
    let ca_pem = ready["ca_pem"].as_str().unwrap();
    assert!(ca_pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
    assert_eq!(get(port("ws"), "/ca.pem").await.text(), ca_pem);

    let mut ws = open_ws(port("ws"), "/echo").await;
    roundtrip(&mut ws, Message::text("hi")).await;

    // Exactly one line: nothing else arrives on stdout before exit.
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "server did not exit on stdin EOF"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut rest).unwrap();
    assert_eq!(rest, "");
}

#[test]
fn human_output_without_ready_json() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ws_bench_server"))
        .args(["--port", "0", "--tls-port", "0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    assert!(
        line.starts_with("ws  listening on ws://127.0.0.1:"),
        "{line}"
    );
    // stdin EOF does not stop it in this mode.
    drop(child.stdin.take());
    std::thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    let _ = child.wait();
}
