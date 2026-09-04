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
    /// The reconcile pass found a divergence the webhook path missed.
    #[serde(rename = "sync.drift")]
    SyncDrift {
        issue_id: i64,
        key: String,
        field: String,
    },
    /// A user has a new in-app notification. Clients filter by their own id.
    #[serde(rename = "notified")]
    Notified { user_id: i64 },
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
        }
    }
}
