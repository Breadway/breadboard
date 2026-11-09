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

/// Only absolute http(s) URLs may be rendered as links. Anything else (a
/// `javascript:` URL stored by a malicious agent) is rejected at the edge so
/// the web client never has to trust it.
/// Collapse an agent one-liner to a single trimmed line. Empty clears it.
pub fn normalize_agent_message(s: &str) -> Result<Option<String>, String> {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return Ok(None);
    }
    if line.chars().count() > 120 {
        return Err("message must be 120 characters or fewer".into());
    }
    Ok(Some(line))
}

pub fn is_safe_http_url(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("http://") || s.starts_with("https://"))
        && s.len() > 8
        && !s.chars().any(|c| c.is_whitespace())
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
    #[serde(default)]
    pub project_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CyclePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<i64>>,
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
    /// `exclusive` (default) or `intent`. An intent reserves interest but
    /// does not move the card or block another agent's exclusive claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<crate::ClaimMode>,
    /// Optional one-line message for this agent (shown under its name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
        if self.agent_name.trim().len() > 64 {
            return Err("agent_name must be 64 chars or fewer".into());
        }
        if let Some(url) = &self.pr_url {
            if !is_safe_http_url(url) {
                return Err("pr_url must be an absolute http(s) URL".into());
            }
        }
        if let Some(message) = &self.message {
            normalize_agent_message(message)?;
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
    /// `null` clears the agent's one-liner; absent leaves it unchanged.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub message: Option<Option<String>>,
}

/// `PUT /api/agents/message` body.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentMessage {
    pub agent_name: String,
    #[serde(default)]
    pub message: String,
}

/// `POST /api/chat/messages`. With `agent_name` the author is that agent
/// (must belong to the caller); without it the author is the signed-in human.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewChatMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Session speaking as `agent_name`. When set, a live name held by a
    /// different session is refused instead of letting two sessions share the
    /// identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Room id or shared-room name (`lobby`). Ignored when `to` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// Agent name to open/send a DM to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Upload ids from `/api/chat/uploads`, attached to this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uploads: Vec<i64>,
}

/// `POST /api/chat/rooms`: create a shared room. The global `lobby` always
/// exists; anything else is fair game.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewChatRoom {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Room names are slugs: lowercase letters, digits, single hyphens.
pub fn normalize_room_name(s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in s.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        return Err("room name must contain letters or digits".into());
    }
    if out.chars().count() > 40 {
        return Err("room name must be 40 chars or fewer".into());
    }
    Ok(out)
}

/// `PUT /api/chat/push-subs`: toggle push for a shared room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatPushSub {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Room name or id.
    pub room: String,
    pub enabled: bool,
}

/// `POST /api/chat/pushes/ack`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckChatPushes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    pub up_to: i64,
}

/// `POST /api/chat/dm`: open (or fetch) a DM with an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenDm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub with: String,
}

/// Server-side roll-up for `GET /api/agents/status`. Counts mirror the
/// opencode `bb-status` widget line (`BB x/y done, a active, …`), computed
/// from live claims instead of local session files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStatusSummary {
    pub total: i64,
    pub live: i64,
    pub stale: i64,
    pub queued: i64,
    pub done: i64,
    pub active: i64,
    pub review: i64,
    pub verifying: i64,
    pub blocked: i64,
    pub remaining: i64,
    pub line: String,
    pub claims: Vec<crate::AgentClaim>,
}

/// `POST /api/agents/register` body. Omit `agent_name` to have the server
/// assign a unique curated name; include it to register or refresh under a
/// name the caller chose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRegister {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// OpenCode parent session id. The breadboard-chat plugin is the reliable
    /// reporter; the roster resolves it to an orchestrator name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Optional orchestrator name. When `parent_session_id` is absent, the
    /// server resolves a known name to its registered session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<String>,
}

/// `POST /api/agents/register` response. `alternatives` carries two or three
/// other free curated names so a caller can pick without rerolling blindly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistration {
    pub agent_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    pub live: bool,
    /// Paused agents stay registered but cannot claim new work.
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_session_id: Option<String>,
}

/// `POST /api/agents/{name}/rename` body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRename {
    pub new_name: String,
}

/// One row of `GET /api/agents/roster`: the union of live sessions,
/// agent profiles and claims, deduplicated by lowercased agent name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRosterEntry {
    pub agent_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    /// True only for a heartbeat inside the last 120 seconds on a name that is
    /// not paused.
    pub live: bool,
    /// Paused agents keep their claims and stay on the roster, but cannot
    /// claim new work and are not woken by chat pushes.
    pub paused: bool,
    /// `live`, `stale` or `paused`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::AgentState>,
    pub claim_count: i64,
    pub exclusive_count: i64,
    pub intent_count: i64,
    /// Live exclusive claim cap for one agent name (default 10).
    pub exclusive_limit: i64,
    /// Live exclusive claims across this agent's orchestrator tree: the root
    /// agent plus every registered subagent below it. An unlinked agent is
    /// its own root, so this equals `exclusive_count`.
    pub orchestrator_exclusive_count: i64,
    /// Live exclusive claim cap for one orchestrator tree (default 100).
    pub orchestrator_exclusive_limit: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_by: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_by_login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Hidden from the default Agents view. Claims and chat identity remain.
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_by: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_by_login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The session that spawned this agent, as reported by the plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Resolved orchestrator agent name, when the parent session is registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<String>,
    /// The orchestrator's session id (the same value as parent_session_id when
    /// resolution succeeded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_session_id: Option<String>,
}

/// `POST /api/agents/{name}/kill` response. Kill removes the name instead of
/// leaving a killed roster row, so there is no roster entry to return.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRemoval {
    pub agent_name: String,
    pub removed: bool,
    pub claims_released: i64,
    pub profile_removed: bool,
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
    /// Page size. Omitted means the server default (500), capped at 2000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Rows to skip, for pagination with `limit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

/// File a review request against an issue and the PR under review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewReviewRequest {
    /// Human login or agent name. Shared records, one field.
    pub reviewer: String,
    /// When omitted the server infers it from a known user, then a known
    /// agent name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_kind: Option<crate::ReviewerKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl NewReviewRequest {
    pub fn validate(&self) -> Result<(), String> {
        non_empty(&self.reviewer)?;
        if self.reviewer.trim().chars().count() > 120 {
            return Err("reviewer must be 120 characters or fewer".into());
        }
        if self.pr_url.is_none() && self.pr_number.is_none() {
            return Err("a review request needs a pr_url or pr_number".into());
        }
        if let Some(url) = &self.pr_url {
            if !is_safe_http_url(url) {
                return Err("pr_url must be an absolute http(s) URL".into());
            }
        }
        if let Some(repo) = &self.pr_repo {
            if !repo.contains('/') || repo.split('/').filter(|s| !s.is_empty()).count() != 2 {
                return Err("pr_repo must be owner/name".into());
            }
        }
        if let Some(note) = &self.note {
            if note.chars().count() > 4000 {
                return Err("note must be 4000 characters or fewer".into());
            }
        }
        Ok(())
    }
}

/// Record a review outcome. `agent_name` is optional identity for an agent
/// reviewer using a human session; humans omit it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewRequestPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::ReviewState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
}

impl ReviewRequestPatch {
    pub fn validate(&self) -> Result<(), String> {
        if self.state.is_none() && self.note.is_none() {
            return Err("nothing to update".into());
        }
        if let Some(note) = &self.note {
            if note.chars().count() > 4000 {
                return Err("note must be 4000 characters or fewer".into());
            }
        }
        Ok(())
    }
}
