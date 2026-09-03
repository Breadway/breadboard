//! breadboard-server entry point.
//!
//! Milestone 1: config load, DB connect + migrate, HTTP server with `/healthz`
//! and the `/events` SSE stream. REST CRUD, OAuth, and the sync engine land in
//! later milestones (see the plan / Documentation.md).

mod bus;
mod config;
mod db;
mod http;

use std::net::SocketAddr;

use anyhow::Context;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::bus::Bus;
use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub bus: Bus,
    pub config: std::sync::Arc<Config>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "breadboard_server=info,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::load().context("loading breadboard.toml")?;
    tracing::info!(listen = %config.listen, db = %config.database_path.display(), "starting breadboard-server");

    if let Some(parent) = config.database_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let db = db::connect(&config.database_url())
        .await
        .context("opening database")?;

    let state = AppState {
        db,
        bus: Bus::new(),
        config: std::sync::Arc::new(config.clone()),
    };

    let app = http::router(state);

    let addr: SocketAddr = config.listen.parse().context("parsing listen address")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
