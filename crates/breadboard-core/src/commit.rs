//! Commit provenance: which agent, session, orchestrator and model produced a
//! commit.
//!
//! The record is keyed by repository and full commit SHA. `model_source` is
//! deliberately explicit: `reported` means the agent supplied the model,
//! `session` means exactly one model is recorded in usage data for this exact
//! session id, and `unknown` means neither could be established. A caller must
//! never substitute "the model that was last active" for an unknown.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How the model in a [`CommitAttribution`] was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// The agent explicitly reported the model when recording the commit.
    Reported,
    /// Exactly one model is recorded in usage data for this exact session.
    Session,
    /// No model could be established; `model` is `None`.
    Unknown,
}

impl ModelSource {
    pub const ALL: [ModelSource; 3] = [
        ModelSource::Reported,
        ModelSource::Session,
        ModelSource::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ModelSource::Reported => "reported",
            ModelSource::Session => "session",
            ModelSource::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == value)
    }
}

/// One commit's provenance. `sha` is the commit that was recorded;
/// `patch_id` and `tree_sha` let a later lookup follow a rebase or squash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitAttribution {
    pub id: i64,
    pub repo: String,
    pub sha: String,
    pub issue_id: Option<i64>,
    pub issue_key: Option<String>,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub orchestrator: Option<String>,
    pub orchestrator_session_id: Option<String>,
    /// Raw model id as reported or stored, without a provider prefix.
    pub model: Option<String>,
    pub provider: Option<String>,
    pub model_source: ModelSource,
    pub patch_id: Option<String>,
    pub tree_sha: Option<String>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// How a lookup found an attribution for the requested commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitMatch {
    /// The exact repository and SHA were recorded.
    Exact,
    /// A different SHA carries the same `git patch-id --stable` (rebase).
    PatchId,
    /// A different SHA produced the same tree (squash of the branch tip).
    TreeSha,
}

impl CommitMatch {
    pub fn as_str(self) -> &'static str {
        match self {
            CommitMatch::Exact => "exact",
            CommitMatch::PatchId => "patch_id",
            CommitMatch::TreeSha => "tree_sha",
        }
    }
}

/// Result of looking up a commit. `attribution` is `None` only when the commit
/// has no recorded provenance and no rewrite match was found; `message` says
/// that plainly instead of leaving a caller to infer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitLookup {
    pub repo: String,
    pub sha: String,
    pub known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<CommitMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<CommitAttribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
