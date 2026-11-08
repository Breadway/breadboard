//! breadboard API server as a library, so integration tests and (later) an
//! embedded mode can build the router without the binary.

pub mod api;
pub mod auth;
pub mod bus;
pub mod config;
pub mod db;
pub mod device;
pub mod dto;
pub mod error;
pub mod forgejo;
pub mod http;
pub mod import;
pub mod notify;
pub mod store;
pub mod summary;
pub mod sync;
pub mod webhook;

use std::sync::Arc;

use crate::bus::Bus;
use crate::config::Config;
use crate::forgejo::Forgejo;

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub bus: Bus,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    pub http: reqwest::Client,
    pub forgejo: Forgejo,
    pub config: Arc<Config>,
    /// Card ids with a create writeback in flight, so the request-path spawn
    /// and the reconcile retry cannot push the same card twice.
    pub writeback_in_flight: Arc<std::sync::Mutex<std::collections::HashSet<i64>>>,
}

impl AppState {
    /// Build state from a ready database + config. Used by the binary and tests.
    pub fn new(db: db::Db, config: Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("breadboard-server/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let forgejo = Forgejo::new(http.clone(), &config.forgejo);
        let (shutdown, _) = tokio::sync::watch::channel(false);
        Ok(Self {
            db,
            bus: Bus::new(),
            shutdown,
            http,
            forgejo,
            config: Arc::new(config),
            writeback_in_flight: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        })
    }
}
