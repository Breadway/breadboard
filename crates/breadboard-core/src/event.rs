//! Events broadcast over the server's SSE stream and (when breadd is present)
//! emitted onto the bread event bus as `bread.board.*`.

use serde::{Deserialize, Serialize};

use crate::Status;

/// A change worth telling every connected client about. Serialized as
/// `{ "type": "card.moved", ... }` on the SSE stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum BoardEvent {
    #[serde(rename = "card.created")]
    CardCreated { id: i64, key: String },
    #[serde(rename = "card.updated")]
    CardUpdated { id: i64, key: String },
    #[serde(rename = "card.moved")]
    CardMoved {
        id: i64,
        key: String,
        from: Status,
        to: Status,
    },
    #[serde(rename = "card.assigned")]
    CardAssigned {
        id: i64,
        key: String,
        assignee_id: Option<i64>,
    },
    #[serde(rename = "card.deleted")]
    CardDeleted { id: i64, key: String },
    #[serde(rename = "cycle.started")]
    CycleStarted { id: i64, name: String, issues: i64 },
    #[serde(rename = "cycle.ended")]
    CycleEnded { id: i64, name: String },
    /// The reconcile pass found a field where Forgejo and breadboard disagree,
    /// i.e. a webhook the fast path missed. `winner` is the side whose value is
    /// authoritative (`"breadboard"` or `"forgejo"`), decided by issue-level
    /// `updated_at`; when Forgejo wins the value is also applied.
    #[serde(rename = "sync.drift")]
    SyncDrift {
        issue_id: i64,
        key: String,
        field: String,
        breadboard_value: String,
        forgejo_value: String,
        winner: String,
        at: String,
    },
    /// A user has a new in-app notification. Clients filter by their own id.
    #[serde(rename = "notified")]
    Notified { user_id: i64 },
    /// An agent claimed an issue (exclusive). Clients re-fetch the Agents
    /// view and tint the claimed card.
    #[serde(rename = "agent.claimed")]
    AgentClaimed {
        id: i64,
        key: String,
        agent: String,
        mode: String,
        session_id: Option<String>,
        state: String,
    },
    /// A claim heartbeat / field update. Carries the full claim so clients can
    /// patch one entry instead of refetching both agent lists.
    #[serde(rename = "agent.updated")]
    AgentUpdated {
        id: i64,
        key: String,
        agent: String,
        mode: String,
        session_id: Option<String>,
        state: String,
        claim: Box<crate::AgentClaim>,
    },
    /// A claim was released. The card returns to its normal colour.
    #[serde(rename = "agent.released")]
    AgentReleased { id: i64, key: String },
    /// An agent was removed: its claims were released and its roster, profile,
    /// hidden flag and chat state were deleted. The name is free again.
    #[serde(rename = "agent.removed")]
    AgentRemoved {
        agent: String,
        removed_by: Option<i64>,
        claims_released: i64,
    },
    /// An agent was paused: claims are kept, but new claims are refused and
    /// chat pushes no longer wake it.
    #[serde(rename = "agent.paused")]
    AgentPaused {
        agent: String,
        paused_by: Option<i64>,
    },
    /// A paused agent was resumed.
    #[serde(rename = "agent.resumed")]
    AgentResumed {
        agent: String,
        resumed_by: Option<i64>,
    },
    /// An agent was hidden from the default Agents view. Its claims and chat
    /// identity are untouched.
    #[serde(rename = "agent.hidden")]
    AgentHidden {
        agent: String,
        hidden_by: Option<i64>,
    },
    /// A hidden agent was revealed again.
    #[serde(rename = "agent.unhidden")]
    AgentUnhidden {
        agent: String,
        unhidden_by: Option<i64>,
    },
    /// A chat message was posted to a room or DM.
    #[serde(rename = "chat.message")]
    ChatMessage { room_id: i64, id: i64 },
    /// A shared room or DM was deleted; clients refetch the room list.
    #[serde(rename = "chat.room.deleted")]
    ChatRoomDeleted { id: i64 },
}

impl BoardEvent {
    /// The `bread.board.*` event name this maps to on the bread bus.
    pub fn bread_event_name(&self) -> &'static str {
        match self {
            BoardEvent::CardCreated { .. } => "bread.board.card.created",
            BoardEvent::CardUpdated { .. } => "bread.board.card.updated",
            BoardEvent::CardMoved { .. } => "bread.board.card.moved",
            BoardEvent::CardAssigned { .. } => "bread.board.card.assigned",
            BoardEvent::CardDeleted { .. } => "bread.board.card.deleted",
            BoardEvent::CycleStarted { .. } => "bread.board.cycle.started",
            BoardEvent::CycleEnded { .. } => "bread.board.cycle.ended",
            BoardEvent::SyncDrift { .. } => "bread.board.sync.drift",
            BoardEvent::Notified { .. } => "bread.board.notified",
            BoardEvent::AgentClaimed { .. } => "bread.board.agent.claimed",
            BoardEvent::AgentUpdated { .. } => "bread.board.agent.updated",
            BoardEvent::AgentReleased { .. } => "bread.board.agent.released",
            BoardEvent::AgentRemoved { .. } => "bread.board.agent.removed",
            BoardEvent::AgentPaused { .. } => "bread.board.agent.paused",
            BoardEvent::AgentResumed { .. } => "bread.board.agent.resumed",
            BoardEvent::AgentHidden { .. } => "bread.board.agent.hidden",
            BoardEvent::AgentUnhidden { .. } => "bread.board.agent.unhidden",
            BoardEvent::ChatMessage { .. } => "bread.board.chat.message",
            BoardEvent::ChatRoomDeleted { .. } => "bread.board.chat.room.deleted",
        }
    }
}
