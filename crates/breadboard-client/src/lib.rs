//! Thin async client for `breadboard-server`. Shared by `breadboard-cli` (CLI + TUI).
//!
//! Milestone 1 exposes only `health`; issue/board/cycle methods are added
//! alongside the server's REST routes.

use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("not authenticated — run `breadboard auth login`")]
    Unauthenticated,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub db: bool,
}

#[derive(Clone)]
pub struct Client {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::new(),
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let b = self.http.request(method, format!("{}{path}", self.base_url));
        match &self.token {
            Some(t) => b.bearer_auth(t),
            None => b,
        }
    }

    async fn json<T: DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
        let resp = rb.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthenticated);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Status { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn health(&self) -> Result<Health> {
        self.json(self.req(reqwest::Method::GET, "/healthz")).await
    }
}
