//! breadboard-server entry point.
//!
//! Milestone 2: config + DB, `/healthz`, `/events` SSE, REST CRUD for
//! projects / epics / cycles / issues, and Forgejo OAuth2 login. The Forgejo
//! sync engine (webhooks, reconcile, writeback) lands in milestone 4.

use std::net::SocketAddr;

use anyhow::Context;
use breadboard_server::{auth, config::Config, db, http, notify, store, sync, AppState};
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
    store::ensure_seed(&db).await?;

    if auth::dev_login_enabled() {
        tracing::warn!("BREADBOARD_DEV_LOGIN=1: /auth/dev-login is enabled (insecure, dev only)");
    }

    let listen = config.listen.clone();
    let state = AppState::new(db, config).context("building app state")?;

    if state.forgejo.is_configured() {
        sync::spawn_reconcile(state.clone());
        tracing::info!(
            interval_mins = state.config.forgejo.reconcile_interval_mins,
            "forgejo reconcile loop started"
        );
    } else {
        tracing::info!("forgejo.api_token unset: sync disabled");
    }

    if state.config.notifications.daily_digest {
        notify::spawn_daily_digest(state.clone());
        tracing::info!(
            hour = state.config.notifications.digest_hour,
            "daily digest job started"
        );
    }

    let shutdown = state.shutdown.clone();
    let app = http::router(state);

    let addr: SocketAddr = listen.parse().context("parsing listen address")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .context("server error")?;

    Ok(())
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
