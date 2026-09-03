//! Request/response shapes for the REST API. Kept here (not in `breadboard-core`)
//! because they are HTTP-shaped, not domain types.

use breadboard_core::{IssueLink, Priority, Status};
use serde::Deserialize;

fn non_empty(s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        Err("must not be empty".into())
    } else {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct NewProject {
    pub key: String,
    pub name: String,
    #[serde(default)]
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

#[derive(Debug, Deserialize)]
pub struct NewEpic {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct NewCycle {
    pub name: String,
    pub starts_on: chrono::NaiveDate,
    pub ends_on: chrono::NaiveDate,
}

/// Only `title` is required — the "create a card with just a title" rule.
#[derive(Debug, Deserialize)]
pub struct NewIssue {
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Project key (`BREAD`) or id. Defaults to the first non-archived project.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub estimate: Option<i64>,
    #[serde(default)]
    pub assignee_id: Option<i64>,
    #[serde(default)]
    pub epic_id: Option<i64>,
    #[serde(default)]
    pub cycle_id: Option<i64>,
    #[serde(default)]
    pub labels: Vec<String>,
}

impl NewIssue {
    pub fn validate(&self) -> Result<(), String> {
        non_empty(&self.title)
    }
}

/// Partial update. A field present as `Some` is written; absent is left alone.
/// Nullable fields use a nested `Option<Option<T>>` so `null` clears them.
#[derive(Debug, Default, Deserialize)]
pub struct IssuePatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default, deserialize_with = "double_option")]
    pub estimate: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub assignee_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub epic_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub cycle_id: Option<Option<i64>>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub links: Option<Vec<IssueLink>>,
}

/// Distinguish "key absent" from "key present with value null".
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

#[derive(Debug, Default, Deserialize)]
pub struct IssueFilter {
    pub project: Option<String>,
    pub status: Option<Status>,
    pub cycle_id: Option<i64>,
    pub epic_id: Option<i64>,
    pub assignee_id: Option<i64>,
    #[serde(default)]
    pub mine: bool,
}
