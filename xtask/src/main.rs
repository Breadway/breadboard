//! Repo automation. Mirrors the `xtask` pattern in `bread/`.
//!
//! `cargo run -p xtask -- check-docs`: fail CI if the API/event contract in
//! `Documentation.md` has drifted from the code.

use std::path::Path;

use breadboard_core::BoardEvent;
use breadboard_core::Status;

fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "check-docs" => check_docs(),
        other => {
            eprintln!("unknown xtask: {other:?}\n\nusage: cargo run -p xtask -- check-docs");
            std::process::exit(2);
        }
    }
}

fn check_docs() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let docs = std::fs::read_to_string(root.join("Documentation.md"))?;

    let mut missing: Vec<String> = Vec::new();

    // Every bread.board.* event name must be documented.
    for name in all_event_names() {
        if !docs.contains(name) {
            missing.push(format!(
                "event `{name}` is not mentioned in Documentation.md"
            ));
        }
    }

    // Every workflow status must be listed.
    for s in Status::ALL {
        if !docs.contains(s.as_str()) {
            missing.push(format!(
                "status `{}` is not mentioned in Documentation.md",
                s.as_str()
            ));
        }
    }

    if missing.is_empty() {
        println!("check-docs: OK");
        Ok(())
    } else {
        for m in &missing {
            eprintln!("check-docs: {m}");
        }
        std::process::exit(1);
    }
}

fn all_event_names() -> Vec<&'static str> {
    use breadboard_core::event::BoardEvent::*;
    // One sample of each variant, purely to reach `bread_event_name()`.
    let samples: Vec<BoardEvent> = vec![
        CardCreated {
            id: 0,
            key: String::new(),
        },
        CardUpdated {
            id: 0,
            key: String::new(),
        },
        CardMoved {
            id: 0,
            key: String::new(),
            from: Status::Backlog,
            to: Status::Todo,
        },
        CardAssigned {
            id: 0,
            key: String::new(),
            assignee_id: None,
        },
        CardDeleted {
            id: 0,
            key: String::new(),
        },
        AgentClaimed {
            id: 0,
            key: String::new(),
            agent: String::new(),
            mode: String::new(),
            session_id: None,
            state: String::new(),
        },
        AgentUpdated {
            id: 0,
            key: String::new(),
            agent: String::new(),
            mode: String::new(),
            session_id: None,
            state: String::new(),
        },
        AgentReleased {
            id: 0,
            key: String::new(),
        },
        ChatMessage {
            room_id: 0,
            id: 0,
        },
        CycleStarted {
            id: 0,
            name: String::new(),
            issues: 0,
        },
        CycleEnded {
            id: 0,
            name: String::new(),
        },
        SyncDrift {
            issue_id: 0,
            key: String::new(),
            field: String::new(),
            breadboard_value: String::new(),
            forgejo_value: String::new(),
            winner: String::new(),
            at: String::new(),
        },
        Notified { user_id: 0 },
    ];
    samples.iter().map(|e| e.bread_event_name()).collect()
}
