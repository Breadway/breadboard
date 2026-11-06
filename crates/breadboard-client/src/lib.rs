//! Async client for `breadboard-server`. Shared by `breadboard-cli` (CLI + TUI).

use breadboard_core::dto::{
    EpicPatch, IssueFilter, IssuePatch, NewCycle, NewEpic, NewIssue, NewProject, ProjectPatch,
};
use breadboard_core::{Comment, Cycle, Epic, Issue, Notification, Project, User};
use std::collections::BTreeMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("transport: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("not authenticated: run `breadboard auth login`")]
    Unauthenticated,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub db: bool,
    #[serde(default)]
    pub dev_login: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub issue: Issue,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpicView {
    #[serde(flatten)]
    pub epic: Epic,
    pub progress: BTreeMap<String, i64>,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivityEntry {
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub source: String,
    pub actor: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Inbox {
    pub unread: i64,
    pub notifications: Vec<Notification>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportResult {
    pub epics_created: Vec<String>,
    pub issues_created: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoView {
    pub repo: String,
    pub project_key: Option<String>,
    pub enabled: bool,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: i64,
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub enum DevicePoll {
    Pending,
    Approved(String),
    Expired,
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

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn build(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let b = self.http.request(method, format!("{}{path}", self.base_url));
        match &self.token {
            Some(t) => b.bearer_auth(t),
            None => b,
        }
    }

    async fn send<T: DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
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

    async fn send_empty(&self, rb: reqwest::RequestBuilder) -> Result<()> {
        let resp = rb.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthenticated);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Status { status, body });
        }
        Ok(())
    }

    // --- health / identity ------------------------------------------------

    pub async fn health(&self) -> Result<Health> {
        self.send(self.build(reqwest::Method::GET, "/healthz")).await
    }

    pub async fn me(&self) -> Result<User> {
        self.send(self.build(reqwest::Method::GET, "/api/me")).await
    }

    // --- issues ---------------------------------------------------------

    pub async fn list_issues(&self, filter: &IssueFilter) -> Result<Vec<Issue>> {
        let mut q: Vec<(String, String)> = Vec::new();
        if let Some(p) = &filter.project {
            q.push(("project".into(), p.clone()));
        }
        if let Some(s) = filter.status {
            q.push(("status".into(), s.as_str().into()));
        }
        if let Some(c) = filter.cycle_id {
            q.push(("cycle_id".into(), c.to_string()));
        }
        if let Some(e) = filter.epic_id {
            q.push(("epic_id".into(), e.to_string()));
        }
        if filter.mine {
            q.push(("mine".into(), "true".into()));
        }
        self.send(self.build(reqwest::Method::GET, "/api/issues").query(&q))
            .await
    }

    pub async fn get_issue(&self, key: &str) -> Result<IssueDetail> {
        self.send(self.build(reqwest::Method::GET, &format!("/api/issues/{key}")))
            .await
    }

    pub async fn create_issue(&self, new: &NewIssue) -> Result<Issue> {
        self.send(self.build(reqwest::Method::POST, "/api/issues").json(new))
            .await
    }

    pub async fn patch_issue(&self, key: &str, patch: &IssuePatch) -> Result<Issue> {
        self.send(
            self.build(reqwest::Method::PATCH, &format!("/api/issues/{key}"))
                .json(patch),
        )
        .await
    }

    pub async fn delete_issue(&self, key: &str) -> Result<()> {
        self.send_empty(self.build(reqwest::Method::DELETE, &format!("/api/issues/{key}")))
            .await
    }

    pub async fn add_comment(&self, key: &str, body: &str) -> Result<Comment> {
        self.send(
            self.build(reqwest::Method::POST, &format!("/api/issues/{key}/comments"))
                .json(&serde_json::json!({ "body": body })),
        )
        .await
    }

    // --- projects / epics / cycles ------------------------------------

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        self.send(self.build(reqwest::Method::GET, "/api/projects"))
            .await
    }

    pub async fn create_project(&self, new: &NewProject) -> Result<Project> {
        self.send(self.build(reqwest::Method::POST, "/api/projects").json(new))
            .await
    }

    pub async fn update_project(&self, id: i64, patch: &ProjectPatch) -> Result<Project> {
        self.send(
            self.build(reqwest::Method::PATCH, &format!("/api/projects/{id}"))
                .json(patch),
        )
        .await
    }

    pub async fn list_epics(&self) -> Result<Vec<EpicView>> {
        self.send(self.build(reqwest::Method::GET, "/api/epics")).await
    }

    pub async fn create_epic(&self, new: &NewEpic) -> Result<Epic> {
        self.send(self.build(reqwest::Method::POST, "/api/epics").json(new))
            .await
    }

    pub async fn update_epic(&self, id: i64, patch: &EpicPatch) -> Result<Epic> {
        self.send(
            self.build(reqwest::Method::PATCH, &format!("/api/epics/{id}"))
                .json(patch),
        )
        .await
    }

    pub async fn issue_activity(&self, key: &str) -> Result<Vec<ActivityEntry>> {
        self.send(self.build(reqwest::Method::GET, &format!("/api/issues/{key}/activity")))
            .await
    }

    pub async fn inbox(&self, unread_only: bool) -> Result<Inbox> {
        let path = if unread_only {
            "/api/notifications?unread=true"
        } else {
            "/api/notifications"
        };
        self.send(self.build(reqwest::Method::GET, path)).await
    }

    pub async fn mark_all_read(&self) -> Result<()> {
        self.send_empty(self.build(reqwest::Method::POST, "/api/notifications/read-all"))
            .await
    }

    pub async fn import(
        &self,
        project: &str,
        markdown: &str,
        dry_run: bool,
    ) -> Result<ImportResult> {
        self.send(
            self.build(reqwest::Method::POST, "/api/import").json(&serde_json::json!({
                "project": project,
                "markdown": markdown,
                "dry_run": dry_run,
            })),
        )
        .await
    }

    pub async fn list_cycles(&self) -> Result<Vec<Cycle>> {
        self.send(self.build(reqwest::Method::GET, "/api/cycles"))
            .await
    }

    pub async fn create_cycle(&self, new: &NewCycle) -> Result<Cycle> {
        self.send(self.build(reqwest::Method::POST, "/api/cycles").json(new))
            .await
    }

    pub async fn start_cycle(&self, id: i64) -> Result<Cycle> {
        self.send(self.build(reqwest::Method::POST, &format!("/api/cycles/{id}/start")))
            .await
    }

    pub async fn end_cycle(&self, id: i64) -> Result<Cycle> {
        self.send(self.build(reqwest::Method::POST, &format!("/api/cycles/{id}/end")))
            .await
    }

    // --- synced repos --------------------------------------------------

    pub async fn list_repos(&self) -> Result<Vec<RepoView>> {
        self.send(self.build(reqwest::Method::GET, "/api/repos")).await
    }

    pub async fn add_repo(&self, repo: &str, key: &str, name: Option<&str>) -> Result<RepoView> {
        #[derive(Serialize)]
        struct Body<'a> {
            repo: &'a str,
            key: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            name: Option<&'a str>,
        }
        self.send(
            self.build(reqwest::Method::POST, "/api/repos")
                .json(&Body { repo, key, name }),
        )
        .await
    }

    pub async fn remove_repo(&self, repo: &str) -> Result<()> {
        self.send_empty(self.build(reqwest::Method::DELETE, &format!("/api/repos/{repo}")))
            .await
    }

    pub async fn reconcile(&self) -> Result<()> {
        self.send_empty(self.build(reqwest::Method::POST, "/api/sync/reconcile"))
            .await
    }

    // --- agent claims --------------------------------------------------

    pub async fn list_agents(&self) -> Result<Vec<breadboard_core::AgentClaim>> {
        self.send(self.build(reqwest::Method::GET, "/api/agents")).await
    }

    pub async fn agent_status(&self) -> Result<breadboard_core::dto::AgentStatusSummary> {
        self.send(self.build(reqwest::Method::GET, "/api/agents/status"))
            .await
    }

    pub async fn claim_issue(
        &self,
        key: &str,
        claim: &breadboard_core::dto::ClaimUpsert,
    ) -> Result<breadboard_core::AgentClaim> {
        self.send(
            self.build(reqwest::Method::POST, &format!("/api/issues/{key}/claim"))
                .json(claim),
        )
        .await
    }

    pub async fn heartbeat_claim(
        &self,
        key: &str,
        agent: Option<&str>,
        update: &breadboard_core::dto::ClaimUpdate,
    ) -> Result<breadboard_core::AgentClaim> {
        let mut rb = self.build(reqwest::Method::PUT, &format!("/api/issues/{key}/claim"));
        if let Some(agent) = agent {
            rb = rb.query(&[("agent", agent)]);
        }
        self.send(rb.json(update)).await
    }

    pub async fn release_claim(&self, key: &str, agent: Option<&str>) -> Result<()> {
        let mut rb = self.build(reqwest::Method::DELETE, &format!("/api/issues/{key}/claim"));
        if let Some(agent) = agent {
            rb = rb.query(&[("agent", agent)]);
        }
        self.send_empty(rb).await
    }

    /// Release one claim by its id (multi-claim issues, Agents page).
    pub async fn release_claim_by_id(&self, id: i64) -> Result<()> {
        self.send_empty(self.build(reqwest::Method::DELETE, &format!("/api/claims/{id}")))
            .await
    }

    // --- device auth --------------------------------------------------

    pub async fn device_start(&self) -> Result<DeviceStart> {
        self.send(self.build(reqwest::Method::POST, "/auth/device"))
            .await
    }

    pub async fn device_poll(&self, device_code: &str) -> Result<DevicePoll> {
        let resp = self
            .build(reqwest::Method::POST, "/auth/device/token")
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await?;
        match resp.status().as_u16() {
            200 => {
                #[derive(Deserialize)]
                struct T {
                    access_token: String,
                }
                let t: T = resp.json().await?;
                Ok(DevicePoll::Approved(t.access_token))
            }
            202 => Ok(DevicePoll::Pending),
            410 => Ok(DevicePoll::Expired),
            _ => {
                let status = resp.status();
                Err(Error::Status {
                    status,
                    body: resp.text().await.unwrap_or_default(),
                })
            }
        }
    }
}
