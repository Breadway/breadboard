//! `breadboard.toml`: every key optional, sane defaults (ecosystem convention).
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
    /// Directory for chat uploads. Defaults to `uploads/` beside the database
    /// (inside the data volume in the container).
    pub upload_dir: Option<PathBuf>,
    /// Directory of the built web SPA to serve at `/`. When unset (or missing),
    /// only the API is served; the Vite dev server handles the UI in dev.
    pub web_dir: Option<PathBuf>,
    pub forgejo: ForgejoConfig,
    pub notifications: NotificationConfig,
    pub summary: SummaryConfig,
    pub usage: UsageConfig,
    pub claims: ClaimBudgetConfig,
}

/// Live exclusive claim budgets. Kept in config, not the database, so the
/// limits can be tuned without a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ClaimBudgetConfig {
    /// Maximum live exclusive claims one agent name may hold at once.
    pub max_exclusive_per_agent: i64,
    /// Maximum live exclusive claims across one orchestrator and every
    /// registered agent below it. An unlinked agent is its own root.
    pub max_exclusive_per_orchestrator: i64,
}

/// The two caps in the form the store uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimLimits {
    pub per_agent: i64,
    pub per_orchestrator: i64,
}

impl Default for ClaimLimits {
    fn default() -> Self {
        Self {
            per_agent: 10,
            per_orchestrator: 100,
        }
    }
}

/// Token/cost statistics imported from OpenCode's local session database.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UsageConfig {
    /// Path to the OpenCode SQLite database (`opencode.db`). Defaults to
    /// `$XDG_DATA_HOME/opencode/opencode.db`. In a container mount the whole
    /// OpenCode data directory read-only (SQLite needs the db, -wal and -shm)
    /// and point this inside it; the importer then stays idle when absent.
    pub opencode_db: Option<PathBuf>,
    /// Minutes between import passes. Minimum 1.
    pub import_interval_mins: u64,
    /// Default spend warning threshold in USD. The stored setting wins once it
    /// has been written.
    pub spend_warn_usd: f64,
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
pub struct SummaryConfig {
    /// OpenAI-compatible chat completions endpoint used to compact a room.
    /// Empty disables compaction with a clear error.
    pub url: String,
    pub model: String,
    /// Optional bearer token for the endpoint.
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    /// Emit `bread.board.*` onto the bread bus via `bread-emit` when available.
    pub bread_events: bool,
    /// Send desktop notifications through breadbar.
    pub breadbar: bool,
    /// Run the daily digest job: once a day, users who opted into a digest get
    /// one summary notification of their unread items instead of per-event
    /// pings. Off by default.
    pub daily_digest: bool,
    /// Local hour (0..=23) the daily digest is sent at. Default 8.
    pub digest_hour: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7280".to_string(),
            public_url: "http://127.0.0.1:7280".to_string(),
            database_path: default_data_dir().join("breadboard.db"),
            upload_dir: None,
            web_dir: None,
            forgejo: ForgejoConfig::default(),
            notifications: NotificationConfig::default(),
            summary: SummaryConfig::default(),
            usage: UsageConfig::default(),
            claims: ClaimBudgetConfig::default(),
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

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            model: "Bonsai2-9B-Q2_0".to_string(),
            token: String::new(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            bread_events: true,
            breadbar: true,
            daily_digest: false,
            digest_hour: 8,
        }
    }
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            opencode_db: default_opencode_db(),
            import_interval_mins: 5,
            spend_warn_usd: 5.0,
        }
    }
}

impl Default for ClaimBudgetConfig {
    fn default() -> Self {
        Self {
            max_exclusive_per_agent: ClaimLimits::default().per_agent,
            max_exclusive_per_orchestrator: ClaimLimits::default().per_orchestrator,
        }
    }
}

/// OpenCode's data directory on the machine running the agent. Missing in the
/// container unless mounted; the importer treats absence as "nothing to do".
fn default_opencode_db() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("opencode").join("opencode.db"))
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
        if let Ok(v) = std::env::var("BREADBOARD_UPLOAD_DIR") {
            self.upload_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("BREADBOARD_WEB_DIR") {
            self.web_dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("BREADBOARD_SUMMARY_URL") {
            self.summary.url = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_SUMMARY_MODEL") {
            self.summary.model = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_SUMMARY_TOKEN") {
            self.summary.token = v;
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
        if let Ok(v) = std::env::var("BREADBOARD_FORGEJO_BASE_URL") {
            self.forgejo.base_url = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_FORGEJO_RECONCILE_INTERVAL_MINS") {
            if let Ok(n) = v.parse() {
                self.forgejo.reconcile_interval_mins = n;
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_NOTIFICATIONS_DAILY_DIGEST") {
            self.notifications.daily_digest = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("BREADBOARD_NOTIFICATIONS_DIGEST_HOUR") {
            if let Ok(n) = v.parse::<u8>() {
                self.notifications.digest_hour = n.min(23);
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_USAGE_OPENCODE_DB") {
            self.usage.opencode_db = if v.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(v))
            };
        }
        if let Ok(v) = std::env::var("BREADBOARD_USAGE_IMPORT_INTERVAL_MINS") {
            if let Ok(n) = v.parse::<u64>() {
                self.usage.import_interval_mins = n.max(1);
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_USAGE_SPEND_WARN_USD") {
            if let Ok(n) = v.parse::<f64>() {
                if n.is_finite() && n >= 0.0 {
                    self.usage.spend_warn_usd = n;
                }
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_CLAIMS_MAX_EXCLUSIVE_PER_AGENT") {
            if let Ok(n) = v.parse::<i64>() {
                self.claims.max_exclusive_per_agent = n.max(1);
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_CLAIMS_MAX_EXCLUSIVE_PER_ORCHESTRATOR") {
            if let Ok(n) = v.parse::<i64>() {
                self.claims.max_exclusive_per_orchestrator = n.max(1);
            }
        }
    }

    /// Where chat attachments are written. Persisted on the same volume as
    /// the database by default.
    pub fn uploads_dir(&self) -> PathBuf {
        self.upload_dir.clone().unwrap_or_else(|| {
            self.database_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("uploads")
        })
    }

    /// SQLite connection string with `?mode=rwc` so the file is created on first run.
    pub fn database_url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.database_path.display())
    }

    /// Active live exclusive claim budgets.
    pub fn claim_limits(&self) -> ClaimLimits {
        ClaimLimits {
            per_agent: self.claims.max_exclusive_per_agent.max(1),
            per_orchestrator: self.claims.max_exclusive_per_orchestrator.max(1),
        }
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
