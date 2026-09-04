//! Notifications: in-app rows (the bell), the `notified` SSE event, and a
//! best-effort `bread-emit` of `bread.board.*` onto a local bread bus when one
//! is reachable (self-hosted-on-your-desktop case; a no-op on a headless host).

use breadboard_core::{BoardEvent, Issue, NotificationKind};

use crate::{store, AppState};

/// Fire `bread-emit <name> <json>` if enabled and the binary is on PATH.
/// Fire-and-forget; never blocks the request path meaningfully.
pub fn emit_bread(state: &AppState, event: &BoardEvent) {
    if !state.config.notifications.bread_events {
        return;
    }
    let name = event.bread_event_name();
    let Ok(payload) = serde_json::to_string(event) else { return };
    let _ = std::process::Command::new("bread-emit")
        .arg(name)
        .arg(payload)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Create a notification row (respecting the user's prefs) and wake their clients.
async fn deliver(
    state: &AppState,
    user_id: i64,
    kind: NotificationKind,
    issue_id: Option<i64>,
    body: String,
) {
    let prefs = store::get_prefs(&state.db, user_id).await.unwrap_or_default();
    let wanted = match kind {
        NotificationKind::Assigned => prefs.assigned,
        NotificationKind::Mention => prefs.mention,
        NotificationKind::CycleStarted | NotificationKind::CycleEnded => prefs.cycle,
    };
    if !wanted {
        return;
    }
    if store::create_notification(&state.db, user_id, kind, issue_id, &body)
        .await
        .is_ok()
    {
        state.bus.publish(BoardEvent::Notified { user_id });
    }
}

/// Issue assigned to someone other than the actor.
pub async fn assigned(state: &AppState, issue: &Issue, new_assignee: i64, actor: Option<i64>) {
    if Some(new_assignee) == actor {
        return;
    }
    deliver(
        state,
        new_assignee,
        NotificationKind::Assigned,
        Some(issue.id),
        format!("{} assigned to you: {}", issue.key, issue.title),
    )
    .await;
}

/// `@login` mentions in a piece of text (comment body or issue description).
pub async fn mentions(state: &AppState, issue: &Issue, text: &str, actor: Option<i64>, ctx: &str) {
    for login in parse_mentions(text) {
        if let Ok(Some(uid)) = store::find_user_by_login(&state.db, &login).await {
            if Some(uid) == actor {
                continue;
            }
            deliver(
                state,
                uid,
                NotificationKind::Mention,
                Some(issue.id),
                format!("mentioned you in {} ({ctx})", issue.key),
            )
            .await;
        }
    }
}

/// Everyone with an assigned issue in a cycle that just started / ended.
pub async fn cycle(state: &AppState, cycle_id: i64, name: &str, started: bool) {
    let kind = if started {
        NotificationKind::CycleStarted
    } else {
        NotificationKind::CycleEnded
    };
    let verb = if started { "started" } else { "ended" };
    if let Ok(users) = store::issue_assignees_in_cycle(&state.db, cycle_id).await {
        for uid in users {
            deliver(state, uid, kind, None, format!("cycle \u{201c}{name}\u{201d} {verb}")).await;
        }
    }
}

fn parse_mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_')
            {
                j += 1;
            }
            if j > start {
                let login = text[start..j].to_string();
                if !out.contains(&login) {
                    out.push(login);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn mention_parsing() {
        assert_eq!(
            super::parse_mentions("hey @riley and @bread-bot, cc @riley"),
            vec!["riley", "bread-bot"]
        );
        assert_eq!(super::parse_mentions("email me@example.com"), Vec::<String>::new());
    }
}
