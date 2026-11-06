//! Request shapes for the REST API: shared by the server and by
//! `breadboard-client` (the CLI/TUI). HTTP-flavoured, but no I/O.

use serde::{Deserialize, Serialize};

use crate::{IssueLink, Priority, Status};

fn non_empty(s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        Err("must not be empty".into())
    } else {
        Ok(())
    }
}

/// Move an issue to a different project: `project` is a key or numeric id,
/// same resolution as `NewIssue::project`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveProject {
    pub project: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProject {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forgejo_repo: Option<String>,
}

impl NewProject {
    pub fn validate(&self) -> Result<(), String> {
        non_empty(&self.key)?;
        non_empty(&self.name)?;
        if !self
            .key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return Err("key must be uppercase ASCII letters/digits, e.g. BREAD".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// `Some(None)` detaches the repo mapping.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub forgejo_repo: Option<Option<String>>,
}

impl ProjectPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.archived.is_none() && self.forgejo_repo.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEpic {
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpicPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCycle {
    pub name: String,
    pub starts_on: chrono::NaiveDate,
    pub ends_on: chrono::NaiveDate,
}

/// Only `title` is required: the "create a card with just a title" rule.
/// `project` is required as soon as there is more than one project, so a
/// title-only card cannot silently land in the wrong tracker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewIssue {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Project key (`BREAD`) or id. Required once more than one project
    /// exists: there is no catch-all default. Use a repo-backed project only
    /// for that repo's code; create a native project for local config,
    /// tooling, or a project that has no repo yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epic_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

impl NewIssue {
    pub fn validate(&self) -> Result<(), String> {
        non_empty(&self.title)
    }
}

/// Partial update. A field present as `Some` is written; absent is left alone.
/// Nullable fields use `Option<Option<T>>` so an explicit `null` clears them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssuePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub estimate: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee_id: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub epic_id: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub cycle_id: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<IssueLink>>,
    /// Sync-managed (Forgejo → breadboard); not pushed back.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub milestone: Option<Option<String>>,
}

impl IssuePatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.status.is_none()
            && self.priority.is_none()
            && self.estimate.is_none()
            && self.assignee_id.is_none()
            && self.epic_id.is_none()
            && self.cycle_id.is_none()
            && self.labels.is_none()
            && self.links.is_none()
            && self.milestone.is_none()
    }
}

/// Distinguish "key absent" from "key present with value null".
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// Upsert body for `POST /api/issues/{key}/claim`. The agent names itself;
/// auth comes from the existing session / bearer token (no separate agent
/// tokens). `state` mirrors the opencode session lifecycle so the Agents
/// page reads like the `bb-status` widget.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaimUpsert {
    /// Human/agent-chosen name, e.g. `opencode`, `codex-hermes`, `bb-worker-2`.
    pub agent_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::AgentState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
}

impl ClaimUpsert {
    pub fn validate(&self) -> Result<(), String> {
        non_empty(&self.agent_name)?;
        if self.agent_name.len() > 64 {
            return Err("agent_name must be 64 chars or fewer".into());
        }
        Ok(())
    }
}

/// Heartbeat / field update for `PUT /api/issues/{key}/claim`. Every field
/// optional; absent = unchanged. Nullable fields use `Option<Option<T>>` so
/// an explicit `null` clears them. Any update refreshes `last_heartbeat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaimUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::AgentState>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub session_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub note: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub worktree: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub repo: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub pr_url: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub pr_number: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub head_sha: Option<Option<String>>,
}

/// Server-side roll-up for `GET /api/agents/status`. Counts mirror the
/// opencode `bb-status` widget line (`BB x/y done, a active, …`), computed
/// from live claims instead of local session files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStatusSummary {
    pub total: i64,
    pub live: i64,
    pub stale: i64,
    pub done: i64,
    pub active: i64,
    pub review: i64,
    pub verifying: i64,
    pub blocked: i64,
    pub remaining: i64,
    pub line: String,
    pub claims: Vec<crate::AgentClaim>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epic_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mine: bool,
}
