//! Shared test harness: boot the real router on an ephemeral port with a
//! throwaway SQLite file and a pre-created bearer token.
#![allow(dead_code)] // each test binary uses a different subset

use std::net::SocketAddr;

use breadboard_server::{
    auth::sha256_hex, config::Config, config::ForgejoConfig, config::UsageConfig, db, http, store,
    AppState,
};
use chrono::Utc;
use sqlx::SqlitePool;

pub const WEBHOOK_SECRET: &str = "test-webhook-secret";

pub struct TestApp {
    pub addr: SocketAddr,
    pub db: SqlitePool,
    pub token: String,
    pub session_token: String,
    pub state: AppState,
    _tmp: tempfile::TempDir,
}

pub async fn spawn() -> TestApp {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    spawn_full(None, None, |_| {}).await
}

/// Spawn with a caller-prepared config, e.g. a pinned approvals operator list.
pub async fn spawn_with_config(mutate: impl FnOnce(&mut Config)) -> TestApp {
    spawn_full_with(None, None, |_| {}, mutate).await
}

pub async fn spawn_with_forgejo(fake: Option<(String, String)>) -> TestApp {
    spawn_full(fake, None, |_| {}).await
}

/// Spawn with explicit live exclusive claim budgets (per agent, per tree).
pub async fn spawn_with_claim_limits(per_agent: i64, per_orchestrator: i64) -> TestApp {
    spawn_full(None, Some((per_agent, per_orchestrator)), |_| {}).await
}

/// Spawn with a prepared usage config (billing modes and allowance tables).
pub async fn spawn_with_usage(mutate: impl FnOnce(&mut UsageConfig)) -> TestApp {
    spawn_full(None, None, mutate).await
}

async fn spawn_full(
    fake: Option<(String, String)>,
    claim_limits: Option<(i64, i64)>,
    usage: impl FnOnce(&mut UsageConfig),
) -> TestApp {
    spawn_full_with(fake, claim_limits, usage, |_| {}).await
}

async fn spawn_full_with(
    fake: Option<(String, String)>,
    claim_limits: Option<(i64, i64)>,
    usage: impl FnOnce(&mut UsageConfig),
    config_mutate: impl FnOnce(&mut Config),
) -> TestApp {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = Config {
        database_path: tmp.path().join("test.db"),
        listen: "127.0.0.1:0".into(),
        public_url: "http://127.0.0.1".into(),
        forgejo: ForgejoConfig {
            webhook_secret: WEBHOOK_SECRET.into(),
            base_url: fake
                .as_ref()
                .map(|(url, _)| url.clone())
                .unwrap_or_else(|| ForgejoConfig::default().base_url),
            api_token: fake.map(|(_, token)| token).unwrap_or_default(),
            ..ForgejoConfig::default()
        },
        ..Config::default()
    };
    if let Some((per_agent, per_orchestrator)) = claim_limits {
        config.claims.max_exclusive_per_agent = per_agent;
        config.claims.max_exclusive_per_orchestrator = per_orchestrator;
    }
    usage(&mut config.usage);
    config_mutate(&mut config);

    let db = db::connect(&config.database_url()).await.unwrap();
    store::ensure_seed(&db).await.unwrap();

    // A user + bearer token, bypassing OAuth.
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (1, 'tester', 'Tester') RETURNING id",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    let token = "test-token-abcdef".to_string();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'test', ?2, ?3)",
    )
    .bind(user_id)
    .bind(sha256_hex(&token))
    .bind(Utc::now().to_rfc3339())
    .execute(&db)
    .await
    .unwrap();

    let session_token = "test-session-abcdef".to_string();
    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(user_id)
    .bind(sha256_hex(&session_token))
    .bind(Utc::now().to_rfc3339())
    .bind((Utc::now() + chrono::Duration::days(1)).to_rfc3339())
    .execute(&db)
    .await
    .unwrap();

    let state = AppState::new(db.clone(), config).unwrap();
    let app = http::router(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestApp {
        addr,
        db,
        token,
        session_token,
        state,
        _tmp: tmp,
    }
}

/// Hex HMAC-SHA256 of `body` with the test webhook secret.
pub fn sign(body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Minimal SSE reader: pulls decoded `type` fields off `/events` in order.
pub struct EventStream {
    resp: reqwest::Response,
    buf: String,
}

impl EventStream {
    pub async fn connect_with_auth(base: &str, authorization: &str) -> Self {
        let resp = reqwest::Client::new()
            .get(format!("{base}/events"))
            .header("authorization", authorization)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        // Give the broadcast subscription a beat to register.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Self {
            resp,
            buf: String::new(),
        }
    }

    pub async fn connect_with_cookie(base: &str, session_token: &str) -> Self {
        let resp = reqwest::Client::new()
            .get(format!("{base}/events"))
            .header("cookie", format!("bb_session={session_token}"))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Self {
            resp,
            buf: String::new(),
        }
    }

    pub async fn next_type(&mut self) -> String {
        let v = self.next_event().await;
        v["type"].as_str().unwrap().to_string()
    }

    pub async fn next_event(&mut self) -> serde_json::Value {
        loop {
            if let Some(idx) = self.buf.find("\n\n") {
                let frame: String = self.buf.drain(..idx + 2).collect();
                for line in frame.lines() {
                    if let Some(data) = line.strip_prefix("data:") {
                        return serde_json::from_str(data.trim()).unwrap();
                    }
                }
                continue;
            }
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(3), self.resp.chunk())
                .await
                .expect("timed out waiting for an SSE event")
                .unwrap()
                .expect("SSE stream ended");
            self.buf.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}
