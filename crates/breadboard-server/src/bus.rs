//! In-process fan-out of [`BoardEvent`]s to every connected SSE client.
//!
//! Milestone 7 adds a second subscriber here that forwards events to
//! `bread-emit` (`bread.board.*`) and breadbar.

use breadboard_core::BoardEvent;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<BoardEvent>,
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self { tx }
    }

    /// Publish an event. It is not an error if there are no subscribers.
    pub fn publish(&self, event: BoardEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BoardEvent> {
        self.tx.subscribe()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}
