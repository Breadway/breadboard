//! breadboard-server entry point.
//!
//! Milestone 2: config + DB, `/healthz`, `/events` SSE, REST CRUD for
//! projects / epics / cycles / issues, and Forgejo OAuth2 login. The Forgejo
//! sync engine (webhooks, reconcile, writeback) lands in milestone 4.

use std::net::SocketAddr;
use std::time::Instant;

use anyhow::Context;
use breadboard_server::{
    agent_events, auth, config::Config, db, http, notify, reviews, store, sync, usage, AppState,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "breadboard_server=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let started = Instant::now();
    let config = Config::load().context("loading breadboard.toml")?;
    tracing::info!(
        listen = %config.listen,
        db = %config.database_path.display(),
        "starting breadboard-server"
    );

    if let Some(parent) = config.database_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let db = db::connect(&config.database_url())
        .await
        .context("opening database")?;
    db::spawn_wal_maintenance(db.clone(), config.database_path.clone());
    tracing::info!(
        elapsed_ms = elapsed_ms(started),
        "startup stage: database ready"
    );
    store::ensure_seed(&db).await?;
    tracing::info!(
        elapsed_ms = elapsed_ms(started),
        "startup stage: seed checked"
    );

    if auth::dev_login_enabled() {
        tracing::warn!("BREADBOARD_DEV_LOGIN=1: /auth/dev-login is enabled (insecure, dev only)");
    }

    let listen = config.listen.clone();
    let state = AppState::new(db, config).context("building app state")?;
    tracing::info!(
        elapsed_ms = elapsed_ms(started),
        "startup stage: app state built"
    );

    if let Err(error) = breadboard_server::artifacts::purge_expired(&state).await {
        tracing::warn!(%error, "artifact expiry on startup failed");
    }
    breadboard_server::artifacts::spawn_expiry_sweep(state.clone());

    if state.forgejo.is_configured() {
        sync::spawn_reconcile(state.clone());
        tracing::info!(
            interval_mins = state.config.forgejo.reconcile_interval_mins,
            "forgejo reconcile loop started"
        );
    } else {
        tracing::info!("forgejo.api_token unset: sync disabled");
    }

    agent_events::spawn_pruner(state.clone());
    tracing::info!(
        retain_days = agent_events::AGENT_EVENT_RETENTION_DAYS,
        "agent event retention loop started"
    );

    if state.config.notifications.daily_digest {
        notify::spawn_daily_digest(state.clone());
        tracing::info!(
            hour = state.config.notifications.digest_hour,
            "daily digest job started"
        );
    }

    reviews::spawn_escalation(state.clone());
    tracing::info!(
        threshold_mins = reviews::REVIEW_ESCALATION_SECS / 60,
        "review escalation loop started"
    );

    if state.config.usage.opencode_db.is_some() {
        usage::spawn_importer(state.clone());
        tracing::info!(
            interval_mins = state.config.usage.import_interval_mins,
            "usage importer started"
        );
    } else {
        tracing::info!("usage.opencode_db unset: token stats disabled");
    }

    let shutdown = state.shutdown.clone();
    let app = http::router(state);
    tracing::info!(
        elapsed_ms = elapsed_ms(started),
        "startup stage: router built"
    );

    let addr: SocketAddr = listen.parse().context("parsing listen address")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let bound = listener.local_addr().context("local_addr after bind")?;
    tracing::info!(
        requested = %addr,
        %bound,
        elapsed_ms = elapsed_ms(started),
        "listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .context("server error")?;

    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn shutdown_signal(shutdown: tokio::sync::watch::Sender<bool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate = signal(SignalKind::terminate()).expect("register SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }

    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.ok();

    tracing::info!("shutdown signal received");
    shutdown.send_replace(true);
}
