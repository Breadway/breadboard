//! Approval asks, their conversation and maintenance windows.
//!
//! These are the shared records between the server (state machine and REST),
//! the MCP tools agents call, and the web inbox. The product rules are enforced
//! in `breadboard-server::approvals`: an agent owns the ask, only a human
//! operator decides, a decision is one-shot and a changes request returns the
//! same ask as a new revision.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Status;

/// What the agent is asking for. `maintenance` asks carry a proposed window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Approval,
    Info,
    Maintenance,
}

impl ApprovalKind {
    pub const ALL: [ApprovalKind; 3] = [
        ApprovalKind::Approval,
        ApprovalKind::Info,
        ApprovalKind::Maintenance,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalKind::Approval => "approval",
            ApprovalKind::Info => "info",
            ApprovalKind::Maintenance => "maintenance",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalUrgency {
    Low,
    Normal,
    High,
}

impl ApprovalUrgency {
    pub const ALL: [ApprovalUrgency; 3] = [
        ApprovalUrgency::Low,
        ApprovalUrgency::Normal,
        ApprovalUrgency::High,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalUrgency::Low => "low",
            ApprovalUrgency::Normal => "normal",
            ApprovalUrgency::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s.trim())
    }
}

/// Ask lifecycle. `changes` is not a resolution: the owning agent resubmits the
/// same row and it returns to `pending` with a higher revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Declined,
    Changes,
    Answered,
    Withdrawn,
    Expired,
}

impl ApprovalStatus {
    pub const ALL: [ApprovalStatus; 7] = [
        ApprovalStatus::Pending,
        ApprovalStatus::Approved,
        ApprovalStatus::Declined,
        ApprovalStatus::Changes,
        ApprovalStatus::Answered,
        ApprovalStatus::Withdrawn,
        ApprovalStatus::Expired,
    ];

    /// Whether the operator still has to act. Only pending asks show in
    /// "Needs you" and count toward the navigation badge.
    pub fn needs_operator(self) -> bool {
        matches!(self, ApprovalStatus::Pending)
    }

    /// Whether an agent may revise or withdraw the ask.
    pub fn is_open_for_agent(self) -> bool {
        matches!(self, ApprovalStatus::Pending | ApprovalStatus::Changes)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Declined => "declined",
            ApprovalStatus::Changes => "changes",
            ApprovalStatus::Answered => "answered",
            ApprovalStatus::Withdrawn => "withdrawn",
            ApprovalStatus::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalEventKind {
    Created,
    Revised,
    Comment,
    Approved,
    Declined,
    Changes,
    Info,
    Withdrawn,
    Expired,
}

impl ApprovalEventKind {
    pub const ALL: [ApprovalEventKind; 9] = [
        ApprovalEventKind::Created,
        ApprovalEventKind::Revised,
        ApprovalEventKind::Comment,
        ApprovalEventKind::Approved,
        ApprovalEventKind::Declined,
        ApprovalEventKind::Changes,
        ApprovalEventKind::Info,
        ApprovalEventKind::Withdrawn,
        ApprovalEventKind::Expired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalEventKind::Created => "created",
            ApprovalEventKind::Revised => "revised",
            ApprovalEventKind::Comment => "comment",
            ApprovalEventKind::Approved => "approved",
            ApprovalEventKind::Declined => "declined",
            ApprovalEventKind::Changes => "changes",
            ApprovalEventKind::Info => "info",
            ApprovalEventKind::Withdrawn => "withdrawn",
            ApprovalEventKind::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalActorKind {
    Agent,
    Human,
    System,
}

impl ApprovalActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalActorKind::Agent => "agent",
            ApprovalActorKind::Human => "human",
            ApprovalActorKind::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "agent" => Some(ApprovalActorKind::Agent),
            "human" => Some(ApprovalActorKind::Human),
            "system" => Some(ApprovalActorKind::System),
            _ => None,
        }
    }
}

/// What the operator can do with a pending ask. `info` closes the ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Decline,
    Changes,
    Info,
}

impl ApprovalDecision {
    pub const ALL: [ApprovalDecision; 4] = [
        ApprovalDecision::Approve,
        ApprovalDecision::Decline,
        ApprovalDecision::Changes,
        ApprovalDecision::Info,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalDecision::Approve => "approve",
            ApprovalDecision::Decline => "decline",
            ApprovalDecision::Changes => "changes",
            ApprovalDecision::Info => "info",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s.trim())
    }

    /// The status the ask moves to. A changes request is the only one that
    /// leaves the door open for the same row to return.
    pub fn status(self) -> ApprovalStatus {
        match self {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Decline => ApprovalStatus::Declined,
            ApprovalDecision::Changes => ApprovalStatus::Changes,
            ApprovalDecision::Info => ApprovalStatus::Answered,
        }
    }

    pub fn event_kind(self) -> ApprovalEventKind {
        match self {
            ApprovalDecision::Approve => ApprovalEventKind::Approved,
            ApprovalDecision::Decline => ApprovalEventKind::Declined,
            ApprovalDecision::Changes => ApprovalEventKind::Changes,
            ApprovalDecision::Info => ApprovalEventKind::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalEvent {
    pub id: i64,
    pub ask_id: i64,
    pub kind: ApprovalEventKind,
    pub actor_kind: ApprovalActorKind,
    pub actor_name: String,
    pub body_md: String,
    pub created_at: DateTime<Utc>,
    /// Free-form context: the revision number, the decision, a revised window.
    /// String values keep the core crate dependency-light.
    pub meta: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAttachment {
    pub id: i64,
    pub ask_id: i64,
    pub artifact_id: i64,
    pub caption: String,
    pub sort: i64,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceStatus {
    Proposed,
    Approved,
    InProgress,
    Completed,
    Cancelled,
}

impl MaintenanceStatus {
    pub const ALL: [MaintenanceStatus; 5] = [
        MaintenanceStatus::Proposed,
        MaintenanceStatus::Approved,
        MaintenanceStatus::InProgress,
        MaintenanceStatus::Completed,
        MaintenanceStatus::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MaintenanceStatus::Proposed => "proposed",
            MaintenanceStatus::Approved => "approved",
            MaintenanceStatus::InProgress => "in_progress",
            MaintenanceStatus::Completed => "completed",
            MaintenanceStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceWindow {
    pub id: i64,
    pub ask_id: Option<i64>,
    pub project_id: Option<i64>,
    pub project_key: Option<String>,
    pub agent_name: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub services: Vec<String>,
    pub reason: String,
    pub risks: String,
    pub rollback: String,
    pub runbook_md: String,
    pub status: MaintenanceStatus,
    pub decided_by: Option<i64>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A change freeze or blackout. Windows inside one are shown as conflicts but
/// never blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreezePeriod {
    pub id: i64,
    pub project_id: Option<i64>,
    pub project_key: Option<String>,
    pub label: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// One ask with the context the inbox and detail view need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAsk {
    pub id: i64,
    pub project_id: Option<i64>,
    pub project_key: Option<String>,
    pub project_name: Option<String>,
    pub issue_id: Option<i64>,
    pub issue_key: Option<String>,
    pub issue_title: Option<String>,
    pub issue_status: Option<Status>,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub kind: ApprovalKind,
    pub urgency: ApprovalUrgency,
    pub title: String,
    pub summary: String,
    pub body_md: String,
    pub environment: Option<String>,
    pub status: ApprovalStatus,
    pub revision: i64,
    pub checklist: Vec<String>,
    pub risk_md: Option<String>,
    pub rollback_md: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub deadline: Option<String>,
    /// True while the newest event is newer than this caller's read cursor.
    pub unread: bool,
    pub decided_by: Option<i64>,
    pub decided_by_login: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decision_note_md: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub events: Vec<ApprovalEvent>,
    pub attachments: Vec<ApprovalAttachment>,
    pub window: Option<MaintenanceWindow>,
}

/// Counts for the navigation badge and the inbox filter chips.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalCounts {
    pub needs: i64,
    pub unread: i64,
    pub all: i64,
    pub approval: i64,
    pub maintenance: i64,
    pub info: i64,
    pub changes: i64,
    pub answered: i64,
}
