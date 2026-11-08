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
    /// An agent was killed: its claims were released and writes from that
    /// name are blocked until it is revived.
    #[serde(rename = "agent.killed")]
    AgentKilled {
        agent: String,
        killed_by: Option<i64>,
    },
    /// A killed agent was explicitly revived. It becomes writable again on its
    /// next heartbeat or registration.
    #[serde(rename = "agent.revived")]
    AgentRevived {
        agent: String,
        revived_by: Option<i64>,
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
            BoardEvent::AgentKilled { .. } => "bread.board.agent.killed",
            BoardEvent::AgentRevived { .. } => "bread.board.agent.revived",
            BoardEvent::ChatMessage { .. } => "bread.board.chat.message",
            BoardEvent::ChatRoomDeleted { .. } => "bread.board.chat.room.deleted",
        }
    }
}
