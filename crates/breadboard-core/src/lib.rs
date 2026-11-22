//! Shared domain types for breadboard: the issue tracker for the bread ecosystem.
//!
//! This crate is deliberately dependency-light (serde + chrono only). It is the
//! contract shared by `breadboard-server`, `breadboard-client`, and the CLI/TUI.
//! Keep it free of I/O, database, and HTTP concerns.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod agent_event;
pub mod commit;
pub mod dto;
pub mod event;

pub use agent_event::{AgentEvent, AgentEventKind, AgentEventPage};
pub use commit::{CommitAttribution, CommitLookup, CommitMatch, ModelSource};
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
    pub project_ids: Vec<i64>,
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
    /// Most recent handoff note left when a claim was released, when one
    /// exists. Embedded so a board card and the issue drawer can show it
    /// without a second request; the full history is `GET
    /// /api/issues/{key}/handoffs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_handoff: Option<HandoffNote>,
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
    /// A review request was filed against you, or its outcome affects you.
    Review,
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
            NotificationKind::Review => "review",
            NotificationKind::Digest => "digest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "assigned" => Some(NotificationKind::Assigned),
            "mention" => Some(NotificationKind::Mention),
            "cycle_started" => Some(NotificationKind::CycleStarted),
            "cycle_ended" => Some(NotificationKind::CycleEnded),
            "review" => Some(NotificationKind::Review),
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

/// A chat room is either the shared lobby or a two-party DM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRoomKind {
    Room,
    Dm,
}

impl ChatRoomKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRoomKind::Room => "room",
            ChatRoomKind::Dm => "dm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "room" => Some(ChatRoomKind::Room),
            "dm" => Some(ChatRoomKind::Dm),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageKind {
    #[default]
    Message,
    /// A compaction summary that stands in for the messages hidden below it.
    Summary,
}

impl ChatMessageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatMessageKind::Message => "message",
            ChatMessageKind::Summary => "summary",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "message" => Some(ChatMessageKind::Message),
            "summary" => Some(ChatMessageKind::Summary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatAuthorKind {
    Agent,
    Human,
}

impl ChatAuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatAuthorKind::Agent => "agent",
            ChatAuthorKind::Human => "human",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(ChatAuthorKind::Agent),
            "human" => Some(ChatAuthorKind::Human),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatUpload {
    pub id: i64,
    pub filename: String,
    pub mime: String,
    pub size: i64,
}

/// The kind of evidence an artifact carries. Stored as the same strings in
/// SQLite; `ci-log` keeps the hyphen so URLs and JSON read naturally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Bench,
    Screenshot,
    CiLog,
    Diff,
    Trace,
    Other,
}

impl ArtifactKind {
    pub const ALL: [ArtifactKind; 6] = [
        ArtifactKind::Bench,
        ArtifactKind::Screenshot,
        ArtifactKind::CiLog,
        ArtifactKind::Diff,
        ArtifactKind::Trace,
        ArtifactKind::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Bench => "bench",
            ArtifactKind::Screenshot => "screenshot",
            ArtifactKind::CiLog => "ci-log",
            ArtifactKind::Diff => "diff",
            ArtifactKind::Trace => "trace",
            ArtifactKind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Images render inline; everything else is shown in the text viewer or
    /// offered as a download.
    pub fn is_image(self) -> bool {
        matches!(self, ArtifactKind::Screenshot)
    }

    /// Logs, diffs and traces are text evidence and get the viewer.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            ArtifactKind::CiLog | ArtifactKind::Diff | ArtifactKind::Trace
        )
    }
}

/// One immutable piece of evidence attached to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueArtifact {
    pub id: i64,
    pub issue_id: i64,
    pub kind: ArtifactKind,
    /// Optional comment this evidence belongs to.
    pub comment_id: Option<i64>,
    /// Optional review request reference (a key or URL). Kept as text so it
    /// stays valid when review requests become their own record.
    pub review_ref: Option<String>,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    /// Agent name that produced it, or `human:<login>` for a web upload.
    pub uploader_agent: String,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub room_id: i64,
    /// `agent:<name>` or `human:<login>`.
    pub author_member: String,
    /// What to show next to the message.
    pub author_display: String,
    pub author_kind: ChatAuthorKind,
    #[serde(default)]
    pub message_kind: ChatMessageKind,
    pub body: String,
    pub attachments: Vec<ChatUpload>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatPushKind {
    /// A message in a DM you belong to.
    Dm,
    /// Someone @mentioned you.
    Ping,
    /// A message in a group room you subscribed to.
    Room,
}

impl ChatPushKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatPushKind::Dm => "dm",
            ChatPushKind::Ping => "ping",
            ChatPushKind::Room => "room",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "dm" => Some(ChatPushKind::Dm),
            "ping" => Some(ChatPushKind::Ping),
            "room" => Some(ChatPushKind::Room),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPush {
    pub id: i64,
    pub kind: ChatPushKind,
    pub room_id: i64,
    pub room_name: Option<String>,
    pub message: ChatMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRoom {
    pub id: i64,
    pub kind: ChatRoomKind,
    /// Set for shared rooms (`lobby`), absent for DMs.
    pub name: Option<String>,
    /// Optional room blurb, e.g. what the silly room is for.
    pub description: Option<String>,
    /// Member keys, e.g. `agent:crumb baron`.
    pub members: Vec<String>,
    /// Messages newer than the reader's cursor, excluding their own.
    pub unread: i64,
    pub last_message: Option<ChatMessage>,
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

/// Whether a claim is exclusive work or a soft intent. Exclusive claims are
/// unique per issue and move the card to `doing`; intents are reservations
/// that show interest without blocking anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimMode {
    #[default]
    Exclusive,
    Intent,
}

impl ClaimMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimMode::Exclusive => "exclusive",
            ClaimMode::Intent => "intent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "exclusive" => Some(ClaimMode::Exclusive),
            "intent" => Some(ClaimMode::Intent),
            _ => None,
        }
    }
}

/// One active agent claim on an issue. At most one *exclusive* claim exists
/// per issue (the first wins; a second agent gets 409 until the first releases
/// or re-claims). Additional `intent` claims may coexist: they reserve
/// interest in queued work without changing the card or blocking another
/// agent. `last_heartbeat` drives the live/stale signal on the Agents page;
/// roster maintenance reaps a claim only when its own heartbeat is past
/// `agents.claim_reap_after_hours` and its `session_id` is null or is not a
/// live session for that agent name, while a hand-paused agent is left alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentClaim {
    pub id: i64,
    pub issue_id: i64,
    pub key: String,
    pub issue_title: String,
    pub issue_status: Status,
    pub claimed_by: i64,
    pub claimed_by_login: String,
    pub agent_name: String,
    /// Optional one-line message shown under the agent's name.
    pub message: Option<String>,
    pub mode: ClaimMode,
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

/// One session that touched an issue. The identifiers are the same ones the
/// roster and usage stats already carry: `session_id` is the OpenCode session
/// and `agent_name` the lowercased agent that claimed it. The row survives a
/// release, so it answers "who worked this before me" after the tint is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSession {
    pub issue_id: i64,
    pub key: String,
    pub title: String,
    pub status: Status,
    pub session_id: String,
    pub agent_name: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// Claim lifecycle state at the last recorded action.
    pub last_state: Option<AgentState>,
}

/// One required release handoff note. Stored apart from the claim so it
/// survives release and can be shown on the card, in the agent timeline and
/// to the next session that picks the issue up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffNote {
    pub id: i64,
    pub issue_id: i64,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub agent_name: String,
    pub mode: ClaimMode,
    pub state: AgentState,
    pub note: String,
    pub created_at: DateTime<Utc>,
}

/// Lifecycle of a review request. `changes_requested` records an outcome but
/// is still outstanding: only `approved` lets the issue be marked done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Requested,
    Accepted,
    ChangesRequested,
    Approved,
}

impl ReviewState {
    pub const ALL: [ReviewState; 4] = [
        ReviewState::Requested,
        ReviewState::Accepted,
        ReviewState::ChangesRequested,
        ReviewState::Approved,
    ];

    /// Whether this state still blocks the issue from being marked done.
    pub fn is_outstanding(self) -> bool {
        !matches!(self, ReviewState::Approved)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReviewState::Requested => "requested",
            ReviewState::Accepted => "accepted",
            ReviewState::ChangesRequested => "changes_requested",
            ReviewState::Approved => "approved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// Whether the reviewer acts as a human user or as a named agent. Both kinds
/// share the same records; the kind only selects the notification path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    Human,
    Agent,
}

impl ReviewerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewerKind::Human => "human",
            ReviewerKind::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human" => Some(ReviewerKind::Human),
            "agent" => Some(ReviewerKind::Agent),
            _ => None,
        }
    }
}

/// A review request: one reviewer on one issue/PR pair. The joined issue
/// fields let a reviewer queue and the requester's issue panel render without
/// a second lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub id: i64,
    pub issue_id: i64,
    pub issue_key: String,
    pub issue_title: String,
    pub issue_status: Status,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub pr_repo: Option<String>,
    pub requested_by: Option<i64>,
    pub requested_by_login: Option<String>,
    pub reviewer_id: Option<i64>,
    pub reviewer: String,
    pub reviewer_kind: ReviewerKind,
    pub state: ReviewState,
    pub note: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub responded_at: Option<DateTime<Utc>>,
    /// Set once when the request sat unresolved past the escalation
    /// threshold. Cleared on a state change.
    pub escalated_at: Option<DateTime<Utc>>,
}
