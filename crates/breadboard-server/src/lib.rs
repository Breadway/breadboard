//! breadboard API server as a library, so integration tests and (later) an
//! embedded mode can build the router without the binary.

pub mod api;
pub mod auth;
pub mod bus;
pub mod config;
pub mod db;
pub mod dto;
pub mod error;
pub mod http;
pub mod store;

use std::sync::Arc;

use crate::bus::Bus;
use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub bus: Bus,
    pub http: reqwest::Client,
    pub config: Arc<Config>,
}

impl AppState {
    /// Build state from a ready database + config. Used by the binary and tests.
    pub fn new(db: db::Db, config: Config) -> anyhow::Result<Self> {
        Ok(Self {
            db,
            bus: Bus::new(),
            http: reqwest::Client::builder()
                .user_agent(concat!("breadboard-server/", env!("CARGO_PKG_VERSION")))
                .build()?,
            config: Arc::new(config),
        })
    }
}
