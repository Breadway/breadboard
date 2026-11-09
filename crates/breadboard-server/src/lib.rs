//! breadboard API server as a library, so integration tests and (later) an
//! embedded mode can build the router without the binary.

pub mod api;
pub mod artifacts;
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
pub mod names;
pub mod notify;
pub mod reviews;
pub mod store;
pub mod summary;
pub mod sync;
pub mod webhook;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
    /// Single-flight guard for reconcile passes: the scheduled loop and the
    /// manual endpoint share it so two passes never run over the same repos.
    pub reconcile: Arc<tokio::sync::Mutex<()>>,
    /// Best-effort read receipts: the last message id already recorded for a
    /// `(room, member)` pair. Fetching messages never waits on the write, and
    /// a repeated fetch that has not advanced does not write again.
    pub read_marks: Arc<Mutex<HashMap<(i64, String), i64>>>,
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
            reconcile: Arc::new(tokio::sync::Mutex::new(())),
            read_marks: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}
