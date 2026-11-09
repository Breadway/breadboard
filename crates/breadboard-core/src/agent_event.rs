//! Per-agent activity timeline types, shared by the server, web and MCP.
//!
//! An event is written where the action already happens, so the feed is a
//! record rather than a reconstruction. Kinds are deliberately small: the
//! summary carries the human detail ("implementing → awaiting-review").

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One kind of action an agent can take on the board.
///
/// `paused` and `removed` exist in the contract now so callers can filter on
/// them; the actions that produce them land with the agent pause/removal
/// work. Until then the feed simply has no rows of those kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    Registered,
    Claimed,
    Released,
    StateChange,
    Paused,
    Removed,
    PrOpened,
    PrMerged,
    CiResult,
    ChatSent,
    Handoff,
}

impl AgentEventKind {
    pub const ALL: [AgentEventKind; 11] = [
        AgentEventKind::Registered,
        AgentEventKind::Claimed,
        AgentEventKind::Released,
        AgentEventKind::StateChange,
        AgentEventKind::Paused,
        AgentEventKind::Removed,
        AgentEventKind::PrOpened,
        AgentEventKind::PrMerged,
        AgentEventKind::CiResult,
        AgentEventKind::ChatSent,
        AgentEventKind::Handoff,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentEventKind::Registered => "registered",
            AgentEventKind::Claimed => "claimed",
            AgentEventKind::Released => "released",
            AgentEventKind::StateChange => "state_change",
            AgentEventKind::Paused => "paused",
            AgentEventKind::Removed => "removed",
            AgentEventKind::PrOpened => "pr_opened",
            AgentEventKind::PrMerged => "pr_merged",
            AgentEventKind::CiResult => "ci_result",
            AgentEventKind::ChatSent => "chat_sent",
            AgentEventKind::Handoff => "handoff",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "registered" => Some(AgentEventKind::Registered),
            "claimed" => Some(AgentEventKind::Claimed),
            "released" => Some(AgentEventKind::Released),
            "state_change" => Some(AgentEventKind::StateChange),
            "paused" => Some(AgentEventKind::Paused),
            "removed" => Some(AgentEventKind::Removed),
            "pr_opened" => Some(AgentEventKind::PrOpened),
            "pr_merged" => Some(AgentEventKind::PrMerged),
            "ci_result" => Some(AgentEventKind::CiResult),
            "chat_sent" => Some(AgentEventKind::ChatSent),
            "handoff" => Some(AgentEventKind::Handoff),
            _ => None,
        }
    }
}

/// One row of the timeline, resolved with the actor's login for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub id: i64,
    pub agent_name: String,
    pub kind: AgentEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i64>,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Login of the human who caused the event, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub at: DateTime<Utc>,
}

/// Cursor-paginated page, newest first. `next_before` is the id to pass as
/// `before` for the next page; it is absent on the last page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEventPage {
    pub events: Vec<AgentEvent>,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before: Option<i64>,
}
