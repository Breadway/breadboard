//! Shared domain types for breadboard: the issue tracker for the bread ecosystem.
//!
//! This crate is deliberately dependency-light (serde + chrono only). It is the
//! contract shared by `breadboard-server`, `breadboard-client`, and the CLI/TUI.
//! Keep it free of I/O, database, and HTTP concerns.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod dto;
pub mod event;

pub use event::BoardEvent;

/// The single global workflow. There are no per-project workflows by design
/// (see the "no configuration sprawl" philosophy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Backlog,
    Todo,
    Doing,
    Review,
    Done,
}

impl Status {
    pub const ALL: [Status; 5] = [
        Status::Backlog,
        Status::Todo,
        Status::Doing,
        Status::Review,
        Status::Done,
    ];

    /// Whether this status maps to a *closed* Forgejo issue. Backlog..=Review
    /// are "open"; only Done is "closed".
    pub fn is_closed(self) -> bool {
        matches!(self, Status::Done)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Todo => "todo",
            Status::Doing => "doing",
            Status::Review => "review",
            Status::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    pub const ALL: [Priority; 5] = [
        Priority::None,
        Priority::Low,
        Priority::Medium,
        Priority::High,
        Priority::Urgent,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Priority::None => "none",
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
            Priority::Urgent => "urgent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    Issue,
    /// Pull requests are mirrored read-mostly; writeback is limited to labels/links.
    Pr,
}

impl IssueType {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueType::Issue => "issue",
            IssueType::Pr => "pr",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "issue" => Some(IssueType::Issue),
            "pr" => Some(IssueType::Pr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkKind {
    Blocks,
    BlockedBy,
    RelatesTo,
    /// A synced PR closes this issue (parsed from its body's closing
    /// keywords, e.g. "Closes #4"); links the two Forgejo objects breadboard
    /// otherwise mirrors as unrelated-looking cards.
    Closes,
    ClosedBy,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Blocks => "blocks",
            LinkKind::BlockedBy => "blocked-by",
            LinkKind::RelatesTo => "relates-to",
            LinkKind::Closes => "closes",
            LinkKind::ClosedBy => "closed-by",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "blocks" => Some(LinkKind::Blocks),
            "blocked-by" => Some(LinkKind::BlockedBy),
            "relates-to" => Some(LinkKind::RelatesTo),
            "closes" => Some(LinkKind::Closes),
            "closed-by" => Some(LinkKind::ClosedBy),
            _ => None,
        }
    }
}

/// Where a mutation originated: used for sync-loop prevention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncSource {
    /// A user acting through a breadboard client.
    Breadboard,
    /// An inbound Forgejo webhook.
    ForgejoWebhook,
    /// The periodic reconcile pass.
    ForgejoReconcile,
}

impl SyncSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncSource::Breadboard => "breadboard",
            SyncSource::ForgejoWebhook => "forgejo_webhook",
            SyncSource::ForgejoReconcile => "forgejo_reconcile",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "breadboard" => Some(SyncSource::Breadboard),
            "forgejo_webhook" => Some(SyncSource::ForgejoWebhook),
            "forgejo_reconcile" => Some(SyncSource::ForgejoReconcile),
            _ => None,
        }
    }
}

/// A breadboard user. Identity is delegated to Forgejo (OAuth2 SSO): a
/// breadboard user *is* a Forgejo user, so assignee/reporter round-trips
/// losslessly through sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub forgejo_id: i64,
    pub login: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

/// A project groups issues under a key prefix. A project with a `forgejo_repo`
/// is synced; one without (e.g. the roadmap) holds breadboard-native items only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    /// Key prefix, e.g. `BREAD`, `BAR`, `ROAD`.
    pub key: String,
    pub name: String,
    /// `owner/repo` on git.breadway.dev, or `None` for a native project.
    pub forgejo_repo: Option<String>,
    /// Next issue number for this project's key counter.
    pub next_number: i64,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Epic {
    pub id: i64,
    /// Own key, e.g. `ROAD-4`. Epics are cross-repo.
    pub key: String,
    pub title: String,
    pub description: Option<String>,
    /// Optional owning project (usually a native one like the roadmap).
    pub project_id: Option<i64>,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A cycle is breadboard's sprint. Cycles are optional; nothing forces an
/// issue into one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cycle {
    pub id: i64,
    pub name: String,
    pub starts_on: chrono::NaiveDate,
    pub ends_on: chrono::NaiveDate,
    pub closed: bool,
}

/// Reference to the Forgejo issue or PR an issue is synced with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgejoRef {
    /// `owner/repo`.
    pub repo: String,
    pub number: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLink {
    pub kind: LinkKind,
    /// Target issue id.
    pub target_id: i64,
}

/// The core entity. `key` (e.g. `BREAD-42`) is breadboard's own counter; it is
/// *not* the Forgejo issue number (that lives in `forgejo_ref`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub key: String,
    pub project_id: i64,
    pub title: String,
    /// Markdown.
    pub body: String,
    pub status: Status,
    pub priority: Priority,
    #[serde(rename = "type")]
    pub issue_type: IssueType,
    /// Optional story points.
    pub estimate: Option<i64>,
    pub assignee_id: Option<i64>,
    pub reporter_id: Option<i64>,
    pub epic_id: Option<i64>,
    pub cycle_id: Option<i64>,
    pub labels: Vec<String>,
    pub links: Vec<IssueLink>,
    pub forgejo_ref: Option<ForgejoRef>,
    /// Forgejo milestone title, synced inbound only.
    pub milestone: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub issue_id: i64,
    pub author_id: Option<i64>,
    pub body: String,
    /// Forgejo comment id, when this comment is synced.
    pub external_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One immutable row of the sync audit trail. Every field-level change from
/// either side is appended here for audit and replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLogEntry {
    pub id: i64,
    pub issue_id: i64,
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub source: SyncSource,
    pub actor_id: Option<i64>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Assigned,
    Mention,
    CycleStarted,
    CycleEnded,
    /// The daily summary of unread items, for users who opted into a digest
    /// instead of per-event pings.
    Digest,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationKind::Assigned => "assigned",
            NotificationKind::Mention => "mention",
            NotificationKind::CycleStarted => "cycle_started",
            NotificationKind::CycleEnded => "cycle_ended",
            NotificationKind::Digest => "digest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "assigned" => Some(NotificationKind::Assigned),
            "mention" => Some(NotificationKind::Mention),
            "cycle_started" => Some(NotificationKind::CycleStarted),
            "cycle_ended" => Some(NotificationKind::CycleEnded),
            "digest" => Some(NotificationKind::Digest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub user_id: i64,
    pub kind: NotificationKind,
    pub issue_id: Option<i64>,
    pub body: String,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

/// Lifecycle state of an agent working an issue. Mirrors the durable
/// `~/.config/breadboard` session states used by the opencode `bb-status`
/// widget, so the server-side Agents page speaks the same language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentState {
    Planned,
    Implementing,
    AwaitingReview,
    Reworking,
    AwaitingCi,
    AwaitingMerge,
    Verifying,
    Blocked,
    NeedsUser,
    Done,
}

impl AgentState {
    pub const ALL: [AgentState; 10] = [
        AgentState::Planned,
        AgentState::Implementing,
        AgentState::AwaitingReview,
        AgentState::Reworking,
        AgentState::AwaitingCi,
        AgentState::AwaitingMerge,
        AgentState::Verifying,
        AgentState::Blocked,
        AgentState::NeedsUser,
        AgentState::Done,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Planned => "planned",
            AgentState::Implementing => "implementing",
            AgentState::AwaitingReview => "awaiting-review",
            AgentState::Reworking => "reworking",
            AgentState::AwaitingCi => "awaiting-ci",
            AgentState::AwaitingMerge => "awaiting-merge",
            AgentState::Verifying => "verifying",
            AgentState::Blocked => "blocked",
            AgentState::NeedsUser => "needs-user",
            AgentState::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// One active agent claim on an issue. At most one claim per issue
/// (exclusive): the first claim wins, a second agent gets 409 until the
/// first releases or re-claims. `last_heartbeat` drives the live/stale
/// signal on the Agents page; claims never auto-expire server-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentClaim {
    pub issue_id: i64,
    pub key: String,
    pub issue_title: String,
    pub issue_status: Status,
    pub claimed_by: i64,
    pub claimed_by_login: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub state: AgentState,
    pub note: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub repo: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub head_sha: Option<String>,
    pub board_status: Status,
    pub claimed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}
