//! `breadboard.toml`: every key optional, sane defaults (ecosystem convention).
//!
//! Search order for the file:
//!   1. `$BREADBOARD_CONFIG` if set
//!   2. `$XDG_CONFIG_HOME/breadboard/breadboard.toml`
//!   3. `~/.config/breadboard/breadboard.toml`
//!
//! Env vars (`BREADBOARD_*`) override file values for secrets in deployment.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    pub agents: AgentRosterConfig,
    pub approvals: ApprovalConfig,
}

/// Approval decisions are the operator's. An empty `operators` list means any
/// authenticated human may decide; listing logins pins the decision to those
/// accounts and refuses every other human as well as any request that
/// presents an agent identity.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    pub operators: Vec<String>,
    pub notifications: ApprovalNotificationConfig,
}

/// Gotify delivery for approval asks. Off unless `enabled` is true and both a
/// url and token are set. The token is the Gotify application token from this
/// config block; the server never reads the infra notification token.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApprovalNotificationConfig {
    pub enabled: bool,
    /// Base Gotify URL, e.g. `http://127.0.0.1:7601`.
    pub url: String,
    /// Gotify application token used to post messages.
    pub token: String,
    /// App name shown in the message title. Defaults to `breadboard`.
    pub app: String,
}

impl Default for ApprovalNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            token: String::new(),
            app: "breadboard".into(),
        }
    }
}

impl ApprovalNotificationConfig {
    /// Whether this config can deliver a message at all.
    pub fn usable(&self) -> bool {
        self.enabled && !self.url.trim().is_empty() && !self.token.trim().is_empty()
    }
}

/// Agent roster lifecycle. Rows nobody is using are pruned by a periodic
/// maintenance pass, after a stale-claim pass has released claims that would
/// otherwise pin them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AgentRosterConfig {
    /// Minutes between periodic maintenance passes, each of which reaps stale
    /// claims and then prunes abandoned rows. `0` disables the periodic pass
    /// but keeps the pass that runs once at startup. Default 60.
    pub maintenance_interval_mins: u64,
    /// Hours without a heartbeat before a session-bound agent row is
    /// considered abandoned. Rows that carry a `session_id` use this window so
    /// subagent history and parent links survive a day. The row must also be
    /// unpaused and unpinned; a pin only guards this automatic pass, so kill
    /// still removes it. `0` disables pruning of session-bound rows. Default 24.
    pub prune_after_hours: u64,
    /// Hours without a heartbeat before a sessionless agent row is removed.
    /// A sessionless row is a one-shot identity with no history: it applies
    /// only when the row also has no claims, no chat membership or DM, no open
    /// review as reviewer, and is not paused or pinned. `0` disables removal of
    /// sessionless rows. Default 2.
    pub ephemeral_prune_after_hours: u64,
    /// Hours without a claim heartbeat before roster maintenance releases it,
    /// so a dead MCP process cannot pin its agent row forever. A claim survives
    /// only when its own `session_id` matches a live session for that agent
    /// name; a live session never protects claims from other sessions that
    /// reused the name. Claims held by a paused agent are never reaped. `0`
    /// disables the pass. Default 72.
    pub claim_reap_after_hours: u64,
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
    /// has been written. Only metered billed cost counts against it.
    pub spend_warn_usd: f64,
    /// Billing mode for every provider without a `providers` entry.
    pub billing_mode: BillingMode,
    /// Per-provider billing, keyed by the provider id OpenCode records
    /// (`opencode-go`, not the `provider/model` label). A provider missing here
    /// uses `billing_mode` with no allowance table.
    pub providers: HashMap<String, ProviderBillingConfig>,
}

/// How a provider charges for tokens.
///
/// `Metered` providers bill per token, so a reported cost is real spend.
/// `Allowance` providers are prepaid plans: each model has a monthly dollar
/// allowance, and usage draws it down at the model's per-token API rates.
/// OpenCode's own reported cost is ignored for these providers because it is
/// inconsistent; consumption is computed from tokens and rates instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingMode {
    #[default]
    Metered,
    Allowance,
}

impl BillingMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" => None,
            "metered" => Some(Self::Metered),
            "allowance" => Some(Self::Allowance),
            _ => None,
        }
    }
}

/// Per-1M-token USD rates. Reasoning tokens are charged at the output rate.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TokenRates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    /// Rates inside the provider's off-peak window. Absent means flat pricing.
    pub off_peak: Option<Box<TokenRates>>,
}

/// One model on an allowance plan.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModelAllowanceConfig {
    /// Monthly dollar allowance for this model under the plan.
    pub allowance_usd: f64,
    /// Peak (standard) rates, with optional off-peak overrides.
    pub rates: TokenRates,
}

/// Billing configuration for one provider.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProviderBillingConfig {
    /// `metered` (default) or `allowance`.
    pub mode: BillingMode,
    /// What the plan costs per month. Used only to show leverage
    /// (monthly allowance / plan price), never to price tokens.
    pub plan_price_usd: f64,
    /// Off-peak window in UTC as `HH:MM`. The window may wrap midnight
    /// (`16:30` to `00:30`). Both ends must be set to take effect; without
    /// them every bucket is charged at the peak rate.
    pub off_peak_start_utc: Option<String>,
    pub off_peak_end_utc: Option<String>,
    /// Allowance table keyed by model id as OpenCode reports it
    /// (`deepseek-v4.1-flash`). Unlisted models count as unrated tokens.
    pub models: HashMap<String, ModelAllowanceConfig>,
}

impl UsageConfig {
    /// Per-provider billing config. Matching is case-insensitive; a
    /// `provider/model` label resolves on its provider prefix so rows that lost
    /// their provider field still price correctly.
    pub fn provider_for(&self, provider: Option<&str>) -> Option<&ProviderBillingConfig> {
        let provider = provider.map(str::trim).filter(|p| !p.is_empty())?;
        if let Some(cfg) = self
            .providers
            .iter()
            .find(|(key, _)| key.trim().eq_ignore_ascii_case(provider))
            .map(|(_, cfg)| cfg)
        {
            return Some(cfg);
        }
        let (prefix, _) = provider.split_once('/')?;
        self.providers
            .iter()
            .find(|(key, _)| key.trim().eq_ignore_ascii_case(prefix))
            .map(|(_, cfg)| cfg)
    }

    /// Billing mode for a provider id. Missing or unknown providers are
    /// metered.
    pub fn billing_for(&self, provider: Option<&str>) -> BillingMode {
        self.provider_for(provider)
            .map(|cfg| cfg.mode)
            .unwrap_or(self.billing_mode)
    }

    /// Merge a TOML provider table, used by `BREADBOARD_USAGE_PROVIDERS_TOML`
    /// so a container can configure allowance tables without a mounted config
    /// file. The text is the body under `[usage.providers]`:
    ///
    /// ```toml
    /// [opencode-go]
    /// mode = "allowance"
    /// ```
    pub fn merge_provider_toml(&mut self, text: &str) -> anyhow::Result<()> {
        let providers: HashMap<String, ProviderBillingConfig> = toml::from_str(text)
            .map_err(|error| anyhow::anyhow!("BREADBOARD_USAGE_PROVIDERS_TOML: {error}"))?;
        self.providers.extend(providers);
        Ok(())
    }
}

impl ProviderBillingConfig {
    /// Allowance entry for a model id, ignoring case and an optional provider
    /// prefix (`opencode-go/deepseek-v4.1-flash`).
    pub fn model_for(&self, model: &str) -> Option<&ModelAllowanceConfig> {
        if let Some(cfg) = self.models.get(model) {
            return Some(cfg);
        }
        let raw = model.rsplit('/').next().unwrap_or(model);
        self.models
            .iter()
            .find(|(key, _)| {
                key.trim().eq_ignore_ascii_case(model) || key.trim().eq_ignore_ascii_case(raw)
            })
            .map(|(_, cfg)| cfg)
    }
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
            agents: AgentRosterConfig::default(),
            approvals: ApprovalConfig::default(),
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
            billing_mode: BillingMode::Metered,
            providers: HashMap::new(),
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

impl Default for AgentRosterConfig {
    fn default() -> Self {
        Self {
            maintenance_interval_mins: 60,
            prune_after_hours: 24,
            ephemeral_prune_after_hours: 2,
            claim_reap_after_hours: 72,
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
        cfg.apply_env()?;
        Ok(cfg)
    }

    fn apply_env(&mut self) -> anyhow::Result<()> {
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
        if let Ok(v) = std::env::var("BREADBOARD_USAGE_BILLING_MODE") {
            if let Some(mode) = BillingMode::parse(&v) {
                self.usage.billing_mode = mode;
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_USAGE_PROVIDERS_TOML") {
            self.usage.merge_provider_toml(&v)?;
        }
        if let Ok(v) = std::env::var("BREADBOARD_APPROVALS_NOTIFICATIONS_ENABLED") {
            self.approvals.notifications.enabled = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("BREADBOARD_APPROVALS_NOTIFICATIONS_URL") {
            self.approvals.notifications.url = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_APPROVALS_NOTIFICATIONS_TOKEN") {
            self.approvals.notifications.token = v;
        }
        if let Ok(v) = std::env::var("BREADBOARD_APPROVALS_NOTIFICATIONS_APP") {
            if !v.trim().is_empty() {
                self.approvals.notifications.app = v;
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_APPROVALS_OPERATORS") {
            self.approvals.operators = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
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
        if let Ok(v) = std::env::var("BREADBOARD_AGENTS_MAINTENANCE_INTERVAL_MINS") {
            if let Ok(n) = v.parse::<u64>() {
                self.agents.maintenance_interval_mins = n;
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_AGENTS_PRUNE_AFTER_HOURS") {
            if let Ok(n) = v.parse::<u64>() {
                self.agents.prune_after_hours = n;
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_AGENTS_EPHEMERAL_PRUNE_AFTER_HOURS") {
            if let Ok(n) = v.parse::<u64>() {
                self.agents.ephemeral_prune_after_hours = n;
            }
        }
        if let Ok(v) = std::env::var("BREADBOARD_AGENTS_CLAIM_REAP_AFTER_HOURS") {
            if let Ok(n) = v.parse::<u64>() {
                self.agents.claim_reap_after_hours = n;
            }
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    const OPENCODE_GO: &str = r#"
[usage.providers.opencode-go]
mode = "allowance"
plan_price_usd = 10.0
off_peak_start_utc = "16:30"
off_peak_end_utc = "00:30"

[usage.providers.opencode-go.models."deepseek-v4.1-flash"]
allowance_usd = 100.0

[usage.providers.opencode-go.models."deepseek-v4.1-flash".rates]
input = 0.30
output = 1.20
cache_read = 0.006
cache_write = 0.30

[usage.providers.opencode-go.models."deepseek-v4.1-flash".rates.off_peak]
input = 0.15
output = 0.60
cache_read = 0.003
cache_write = 0.15
"#;

    #[test]
    fn allowance_table_parses_from_toml() {
        let config: Config = toml::from_str(OPENCODE_GO).unwrap();
        let provider = config.usage.provider_for(Some("opencode-go")).unwrap();
        assert_eq!(provider.mode, BillingMode::Allowance);
        assert_eq!(provider.plan_price_usd, 10.0);
        let model = provider.model_for("deepseek-v4.1-flash").unwrap();
        assert_eq!(model.allowance_usd, 100.0);
        assert_eq!(model.rates.input, 0.30);
        let off_peak = model.rates.off_peak.as_ref().unwrap();
        assert_eq!(off_peak.input, 0.15);
        assert_eq!(off_peak.cache_read, 0.003);
        // The provider prefix on a model label still resolves.
        assert!(provider
            .model_for("opencode-go/deepseek-v4.1-flash")
            .is_some());
    }

    #[test]
    fn billing_defaults_to_metered_when_no_provider_entry_exists() {
        let usage = UsageConfig::default();
        assert_eq!(usage.billing_for(Some("opencode-go")), BillingMode::Metered);
        assert_eq!(usage.billing_for(None), BillingMode::Metered);
        assert!(usage.provider_for(Some("opencode-go")).is_none());
    }

    #[test]
    fn shipped_config_parses_and_marks_opencode_go_as_allowance() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../breadboard.toml");
        let text = std::fs::read_to_string(path).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        let provider = config.usage.provider_for(Some("opencode-go")).unwrap();
        assert_eq!(provider.mode, BillingMode::Allowance);
        assert_eq!(provider.plan_price_usd, 10.0);
        assert!(provider
            .model_for("deepseek-v4.1-flash")
            .is_some_and(|m| m.allowance_usd == 100.0));
        assert!(provider
            .model_for("muse-spark-1.3-contributor")
            .is_some_and(|m| m.allowance_usd == 60.0));
    }

    #[test]
    fn provider_table_merges_from_env_shaped_toml() {
        let mut usage = UsageConfig::default();
        usage
            .merge_provider_toml(
                r#"
[opencode-go]
mode = "allowance"
plan_price_usd = 10.0

[opencode-go.models."deepseek-v4.1-flash"]
allowance_usd = 100.0

[opencode-go.models."deepseek-v4.1-flash".rates]
input = 0.30
output = 1.20
"#,
            )
            .unwrap();
        let provider = usage.provider_for(Some("opencode-go")).unwrap();
        assert_eq!(provider.mode, BillingMode::Allowance);
        assert!(provider.model_for("deepseek-v4.1-flash").is_some());
        assert!(usage.merge_provider_toml("not = [valid").is_err());
    }

    #[test]
    fn billing_mode_parses_known_names_only() {
        assert_eq!(BillingMode::parse("metered"), Some(BillingMode::Metered));
        assert_eq!(
            BillingMode::parse(" Allowance "),
            Some(BillingMode::Allowance)
        );
        assert_eq!(BillingMode::parse(""), None);
        assert_eq!(BillingMode::parse("subscription"), None);
    }
}
