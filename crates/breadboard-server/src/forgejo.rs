//! Thin typed wrapper over the Forgejo REST API (`/api/v1`), used by the sync
//! engine for the reconcile pass and for writeback. Authenticated with the
//! service PAT (`forgejo.api_token`).

use anyhow::{anyhow, Context};
use serde::Deserialize;

use crate::config::ForgejoConfig;

#[derive(Clone)]
pub struct Forgejo {
    http: reqwest::Client,
    base: String,
    token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FgUser {
    pub id: i64,
    pub login: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FgLabel {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FgMilestone {
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FgPullRequestStub {
    #[serde(default)]
    pub merged: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FgIssue {
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// "open" | "closed"
    pub state: String,
    #[serde(default)]
    pub labels: Vec<FgLabel>,
    #[serde(default)]
    pub assignees: Option<Vec<FgUser>>,
    #[serde(default)]
    pub milestone: Option<FgMilestone>,
    #[serde(default)]
    pub user: Option<FgUser>,
    /// Present (non-null) when this "issue" is actually a pull request.
    #[serde(default)]
    pub pull_request: Option<FgPullRequestStub>,
    pub created_at: String,
    pub updated_at: String,
}

impl FgIssue {
    pub fn is_pr(&self) -> bool {
        self.pull_request.is_some()
    }
    pub fn is_closed(&self) -> bool {
        self.state == "closed"
    }
    pub fn label_names(&self) -> Vec<String> {
        self.labels.iter().map(|l| l.name.clone()).collect()
    }
    pub fn first_assignee(&self) -> Option<&FgUser> {
        self.assignees.as_ref().and_then(|a| a.first())
    }
}

/// Fields to PATCH onto a Forgejo issue; `None` = leave untouched.
#[derive(Debug, Default, Clone)]
pub struct FgIssuePatch<'a> {
    pub title: Option<&'a str>,
    pub body: Option<&'a str>,
    /// "open" | "closed"
    pub state: Option<&'a str>,
    /// Assignee logins (empty slice clears assignees).
    pub assignees: Option<&'a [String]>,
}

impl FgIssuePatch<'_> {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.state.is_none()
            && self.assignees.is_none()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FgComment {
    pub id: i64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub user: Option<FgUser>,
    pub created_at: String,
    pub updated_at: String,
}

impl Forgejo {
    pub fn new(http: reqwest::Client, cfg: &ForgejoConfig) -> Self {
        Self {
            http,
            base: format!("{}/api/v1", cfg.base_url.trim_end_matches('/')),
            token: cfg.api_token.clone(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.token.is_empty()
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header("Authorization", format!("token {}", self.token))
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        rb: reqwest::RequestBuilder,
    ) -> anyhow::Result<T> {
        let resp = rb.send().await.context("forgejo request failed")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("forgejo {status}: {body}"));
        }
        serde_json::from_str(&body).with_context(|| format!("decoding forgejo response: {body}"))
    }

    /// Issues **and PRs** updated since `since` (RFC3339), all states, paginated.
    /// Callers branch on [`FgIssue::is_pr`].
    pub async fn issues_since(
        &self,
        owner: &str,
        repo: &str,
        since: Option<&str>,
    ) -> anyhow::Result<Vec<FgIssue>> {
        let mut out = Vec::new();
        for page in 1..=50u32 {
            let mut url =
                format!("/repos/{owner}/{repo}/issues?state=all&limit=50&page={page}");
            if let Some(s) = since {
                url.push_str(&format!("&since={}", urlencoding::encode(s)));
            }
            let batch: Vec<FgIssue> = self.json(self.req(reqwest::Method::GET, &url)).await?;
            let n = batch.len();
            out.extend(batch);
            if n < 50 {
                break;
            }
        }
        Ok(out)
    }

    pub async fn get_issue(&self, owner: &str, repo: &str, number: i64) -> anyhow::Result<FgIssue> {
        self.json(self.req(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}/issues/{number}"),
        ))
        .await
    }

    pub async fn issue_comments(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
    ) -> anyhow::Result<Vec<FgComment>> {
        self.json(self.req(
            reqwest::Method::GET,
            &format!("/repos/{owner}/{repo}/issues/{number}/comments"),
        ))
        .await
    }

    /// PATCH title / body / state / assignees. `None` fields are left untouched.
    pub async fn patch_issue(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        patch: FgIssuePatch<'_>,
    ) -> anyhow::Result<()> {
        let mut payload = serde_json::Map::new();
        if let Some(t) = patch.title {
            payload.insert("title".into(), t.into());
        }
        if let Some(b) = patch.body {
            payload.insert("body".into(), b.into());
        }
        if let Some(s) = patch.state {
            payload.insert("state".into(), s.into());
        }
        if let Some(a) = patch.assignees {
            payload.insert("assignees".into(), serde_json::json!(a));
        }
        if payload.is_empty() {
            return Ok(());
        }
        let _: serde_json::Value = self
            .json(
                self.req(
                    reqwest::Method::PATCH,
                    &format!("/repos/{owner}/{repo}/issues/{number}"),
                )
                .json(&serde_json::Value::Object(payload)),
            )
            .await?;
        Ok(())
    }

    /// Replace the issue's labels by name, creating any that don't exist yet.
    pub async fn replace_labels(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        names: &[String],
    ) -> anyhow::Result<()> {
        let existing: Vec<FgLabel> = self
            .json(self.req(
                reqwest::Method::GET,
                &format!("/repos/{owner}/{repo}/labels?limit=100"),
            ))
            .await?;
        let mut ids = Vec::new();
        for name in names {
            if let Some(l) = existing.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
                ids.push(l.id);
            } else {
                let created: FgLabel = self
                    .json(
                        self.req(
                            reqwest::Method::POST,
                            &format!("/repos/{owner}/{repo}/labels"),
                        )
                        .json(&serde_json::json!({ "name": name, "color": "#8a8a8a" })),
                    )
                    .await?;
                ids.push(created.id);
            }
        }
        let _: serde_json::Value = self
            .json(
                self.req(
                    reqwest::Method::PUT,
                    &format!("/repos/{owner}/{repo}/issues/{number}/labels"),
                )
                .json(&serde_json::json!({ "labels": ids })),
            )
            .await?;
        Ok(())
    }

    /// Create an issue (bare; set labels afterward with [`Self::replace_labels`],
    /// since the create endpoint only takes label *ids*). Returns the new issue.
    pub async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
        assignees: &[String],
    ) -> anyhow::Result<FgIssue> {
        let mut payload = serde_json::json!({ "title": title, "body": body });
        if !assignees.is_empty() {
            payload["assignees"] = serde_json::json!(assignees);
        }
        self.json(
            self.req(
                reqwest::Method::POST,
                &format!("/repos/{owner}/{repo}/issues"),
            )
            .json(&payload),
        )
        .await
    }

    pub async fn create_comment(
        &self,
        owner: &str,
        repo: &str,
        number: i64,
        body: &str,
    ) -> anyhow::Result<FgComment> {
        self.json(
            self.req(
                reqwest::Method::POST,
                &format!("/repos/{owner}/{repo}/issues/{number}/comments"),
            )
            .json(&serde_json::json!({ "body": body })),
        )
        .await
    }
}

/// Split `"owner/repo"` into its parts.
pub fn split_repo(full: &str) -> anyhow::Result<(&str, &str)> {
    full.split_once('/')
        .ok_or_else(|| anyhow!("repo must be `owner/name`, got {full:?}"))
}

/// The `pull_request` object in a `pull_request` webhook payload: same fields
/// we care about as an issue, plus `merged`.
#[derive(Debug, Clone, Deserialize)]
pub struct FgPrObject {
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// "open" | "closed"
    pub state: String,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub labels: Vec<FgLabel>,
    #[serde(default)]
    pub assignees: Option<Vec<FgUser>>,
    #[serde(default)]
    pub milestone: Option<FgMilestone>,
    #[serde(default)]
    pub user: Option<FgUser>,
    pub created_at: String,
    pub updated_at: String,
}

impl FgPrObject {
    /// Present a PR as an [`FgIssue`] so the same apply path handles it.
    pub fn into_fg_issue(self) -> FgIssue {
        FgIssue {
            number: self.number,
            title: self.title,
            body: self.body,
            state: if self.merged { "closed".into() } else { self.state },
            labels: self.labels,
            assignees: self.assignees,
            milestone: self.milestone,
            user: self.user,
            pull_request: Some(FgPullRequestStub { merged: self.merged }),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
