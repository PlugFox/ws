use game_server::{api::AppState, config::AppConfig, create_app};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// After a stop signal, how long requests in flight may finish before the
/// last save and exit. WebSockets never finish on their own: they drop at
/// exit and the clients reconnect to the next server.
const DRAIN: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // `game_server healthcheck`: the container's HEALTHCHECK (the runtime
    // image has no shell or curl).
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthcheck() { 0 } else { 1 });
    }

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "game_server=debug,tower_http=debug,axum=trace".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env();
    let addr = format!("{}:{}", config.host, config.port);

    info!("Starting game server on {}", addr);
    let (router, state) = create_app(config).await?;

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Server listening on http://{}", addr);

    // Stop: SIGTERM (`docker stop`) or Ctrl-C. Announce, stop accepting,
    // let requests in flight finish for up to DRAIN, then save every player.
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            let _ = stop_rx.wait_for(|stop| *stop).await;
        });
    let stop = async {
        stop_signal().await;
        info!("Stop signal: saving players and shutting down");
        state.admin.broadcast("The server is restarting. You will be reconnected in a moment.");
        let report = state.admin.save_all().await;
        info!("Saved {} of {} sessions before the drain", report.saved, report.sessions);
        let _ = stop_tx.send(true);
        tokio::time::sleep(DRAIN).await;
    };
    tokio::select! {
        res = server.into_future() => res?,
        () = stop => {}
    }
    save_on_exit(&state).await;
    Ok(())
}

/// The last save: everything changed during the drain.
async fn save_on_exit(state: &AppState) {
    let report = state.admin.save_all().await;
    if report.failed > 0 {
        warn!("Exit save: {} failed of {} sessions", report.failed, report.sessions);
    }
    info!("Exit save: {} saved, {} sessions; bye", report.saved, report.sessions);
}

/// SIGTERM or Ctrl-C (SIGINT).
async fn stop_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}

/// `GET /health` on this host's `SERVER_PORT` answers 200.
fn healthcheck() -> bool {
    use std::io::{Read, Write};
    let port = std::env::var("SERVER_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8080);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(4)));
    if stream.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n").is_err() {
        return false;
    }
    let mut head = [0u8; 16];
    let n = stream.read(&mut head).unwrap_or(0);
    // "HTTP/1.1 200 OK"
    head[..n].starts_with(b"HTTP/1.") && head[..n].get(9..12) == Some(b"200")
}
