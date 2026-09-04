//! `breadboard.toml` — every key optional, sane defaults (ecosystem convention).
//!
//! Search order for the file:
//!   1. `$BREADBOARD_CONFIG` if set
//!   2. `$XDG_CONFIG_HOME/breadboard/breadboard.toml`
//!   3. `~/.config/breadboard/breadboard.toml`
//!
//! Env vars (`BREADBOARD_*`) override file values for secrets in deployment.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: String,
    /// Public origin, used to build OAuth redirect URLs. e.g.
    /// `https://breadboard.breadway.dev`.
    pub public_url: String,
    /// Path to the SQLite database file.
    pub database_path: PathBuf,
    /// Directory of the built web SPA to serve at `/`. When unset (or missing),
    /// only the API is served — the Vite dev server handles the UI in dev.
    pub web_dir: Option<PathBuf>,
    pub forgejo: ForgejoConfig,
    pub notifications: NotificationConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ForgejoConfig {
    /// Base URL of the Forgejo instance, e.g. `https://git.breadway.dev`.
    pub base_url: String,
    /// OAuth2 application client id / secret (the "Sign in with git.breadway.dev" app).
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    /// Shared secret for verifying inbound webhook HMAC signatures.
    pub webhook_secret: String,
    /// A bot/service token used for API writeback. See memory: forgejo-token
    /// (needs re-mint after the 2026-09 compromise).
    pub api_token: String,
    /// Minutes between reconcile passes.
    pub reconcile_interval_mins: u64,
    /// `owner/repo` entries to sync. Empty = sync nothing until `breadboard repos add`.
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Emit `bread.board.*` onto the bread bus via `bread-emit` when available.
    pub bread_events: bool,
    /// Send desktop notifications through breadbar.
    pub breadbar: bool,
    /// Send a daily digest instead of per-event pings.
    pub daily_digest: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7280".to_string(),
            public_url: "http://127.0.0.1:7280".to_string(),
            database_path: default_data_dir().join("breadboard.db"),
            web_dir: None,
            forgejo: ForgejoConfig::default(),
            notifications: NotificationConfig::default(),
        }
    }
}

impl Default for ForgejoConfig {
    fn default() -> Self {
        Self {
            base_url: "https://git.breadway.dev".to_string(),
            oauth_client_id: String::new(),
            oauth_client_secret: String::new(),
            webhook_secret: String::new(),
            api_token: String::new(),
            reconcile_interval_mins: 10,
            repos: Vec::new(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            bread_events: true,
            breadbar: true,
            daily_digest: false,
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let mut cfg: Config = match config_path() {
            Some(path) if path.exists() => {
                let text = std::fs::read_to_string(&path)?;
                toml::from_str(&text)?
            }
            _ => Config::default(),
        };
        cfg.apply_env();
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("BREADBOARD_LISTEN") {
            self.listen = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_PUBLIC_URL") {
            self.public_url = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_DATABASE_PATH") {
            self.database_path = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("BREADBOARD_WEB_DIR") {
            self.web_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("BREADBOARD_FORGEJO_OAUTH_CLIENT_ID") {
            self.forgejo.oauth_client_id = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_FORGEJO_OAUTH_CLIENT_SECRET") {
            self.forgejo.oauth_client_secret = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_FORGEJO_WEBHOOK_SECRET") {
            self.forgejo.webhook_secret = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_FORGEJO_API_TOKEN") {
            self.forgejo.api_token = v;
        }
    }

    /// SQLite connection string with `?mode=rwc` so the file is created on first run.
    pub fn database_url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.database_path.display())
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("BREADBOARD_CONFIG") {
        return Some(PathBuf::from(explicit));
    }
    dirs::config_dir().map(|d| d.join("breadboard").join("breadboard.toml"))
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| Path::new(".").to_path_buf())
        .join("breadboard")
}
