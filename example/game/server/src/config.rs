use crate::world::{HubConfig, StageConfig};
use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub jwt_secret: Vec<u8>,
    pub geoip_db_path: Option<String>,
    pub access_token_ttl: Duration,
    pub refresh_token_ttl: Duration,
    pub flush_interval: Duration,
    /// Root of the baked asset pack (`example/assets/dist`). `None` disables
    /// the hub; so does a pack that fails to load.
    pub assets_dir: Option<PathBuf>,
    /// Location ids simulated by every hub channel.
    pub hub_maps: Vec<String>,
    /// Where players appear when they join the hub.
    pub hub_entry: String,
    pub hub: HubConfig,
    /// Stage instances (missions). Stage maps are the missions' maps.
    pub stages: StageConfig,
    /// Built web client (`dart compile` output) served at `/`. Optional.
    pub client_dir: Option<PathBuf>,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let host = env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = env::var("SERVER_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080);

        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://game_server.db?mode=rwc".to_string());

        let jwt_secret = match env::var("JWT_SECRET") {
            Ok(s) => s.into_bytes(),
            Err(_) => {
                // Default development fallback key (32 bytes)
                b"insecure_dev_jwt_secret_32bytes!!".to_vec()
            }
        };

        let geoip_db_path = env::var("GEOIP_DB_PATH").ok();

        let access_token_ttl_secs = env::var("ACCESS_TOKEN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600); // 1 hour

        let refresh_token_ttl_secs = env::var("REFRESH_TOKEN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2592000); // 30 days

        let flush_interval_secs = env::var("FLUSH_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30); // 30 seconds

        let assets_dir = match env::var("GAME_ASSETS_DIR") {
            Ok(dir) if dir.is_empty() => None,
            Ok(dir) => Some(PathBuf::from(dir)),
            // Relative to `example/game/server`, where `cargo run` starts.
            Err(_) => Some(PathBuf::from("../../assets/dist")),
        };

        let client_dir = match env::var("GAME_CLIENT_DIR") {
            Ok(dir) if dir.is_empty() => None,
            Ok(dir) => Some(PathBuf::from(dir)),
            Err(_) => Some(PathBuf::from("../client/build/web")),
        };

        let mut hub = HubConfig::default();
        if let Some(cap) = env::var("HUB_CHANNEL_CAPACITY").ok().and_then(|v| v.parse().ok()) {
            hub.capacity = cap;
        }
        if let Some(max) = env::var("HUB_MAX_CHANNELS").ok().and_then(|v| v.parse().ok()) {
            hub.max_channels = max;
        }
        let mut stages = StageConfig::default();
        if let Some(max) = env::var("MAX_STAGES").ok().and_then(|v| v.parse().ok()) {
            stages.max_stages = max;
        }

        Self {
            host,
            port,
            database_url,
            jwt_secret,
            geoip_db_path,
            access_token_ttl: Duration::from_secs(access_token_ttl_secs),
            refresh_token_ttl: Duration::from_secs(refresh_token_ttl_secs),
            flush_interval: Duration::from_secs(flush_interval_secs),
            assets_dir,
            hub_maps: vec!["izlude".into(), "izlude_in".into()],
            hub_entry: "izlude".into(),
            hub,
            stages,
            client_dir,
        }
    }

    /// Configuration tailored for in-memory integration testing.
    pub fn test_config() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
            database_url: "sqlite::memory:".to_string(),
            jwt_secret: b"test_secret_key_for_testing_32b!".to_vec(),
            geoip_db_path: None,
            access_token_ttl: Duration::from_secs(3600),
            refresh_token_ttl: Duration::from_secs(86400),
            flush_interval: Duration::from_secs(5),
            assets_dir: None,
            hub_maps: vec!["izlude".into(), "izlude_in".into()],
            hub_entry: "izlude".into(),
            hub: HubConfig::default(),
            stages: StageConfig::default(),
            client_dir: None,
        }
    }
}
