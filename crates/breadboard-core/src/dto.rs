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
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
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
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewIssue {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Project key (`BREAD`) or id. Defaults to the first non-archived project.
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
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub estimate: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub epic_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub cycle_id: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<IssueLink>>,
    /// Sync-managed (Forgejo → breadboard); not pushed back.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
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
