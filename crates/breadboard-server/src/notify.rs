//! Notifications: in-app rows (the bell), the `notified` SSE event, and a
//! best-effort `bread-emit` of `bread.board.*` onto a local bread bus when one
//! is reachable (self-hosted-on-your-desktop case; a no-op on a headless host).

use std::time::Duration;

use breadboard_core::{BoardEvent, Issue, Notification, NotificationKind};
use chrono::{Datelike, Local, Timelike, Utc};

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
        NotificationKind::Digest => prefs.digest,
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

// --- daily digest ----------------------------------------------------------

/// Spawn the daily digest loop: once a day, at `notifications.digest_hour`
/// local time, send opted-in users one summary of their unread items. Same
/// shape as `sync::spawn_reconcile`: a `tokio::time::interval` tick, guarded so
/// it fires at most once per calendar day.
pub fn spawn_daily_digest(state: AppState) {
    let hour = u32::from(state.config.notifications.digest_hour.min(23));
    tokio::spawn(async move {
        // Check every 10 minutes so a missed tick (sleep, load) still lands in
        // the target hour.
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        let mut last_run: Option<i32> = None;
        loop {
            tick.tick().await;
            let now = Local::now();
            let ordinal = now.year() * 366 + now.ordinal() as i32;
            if now.hour() == hour && last_run != Some(ordinal) {
                last_run = Some(ordinal);
                send_daily_digest(&state).await;
            }
        }
    });
}

/// Send the digest to every opted-in user with pending items. Public for the
/// scheduled loop and for tests.
pub async fn send_daily_digest(state: &AppState) {
    let recipients = match store::digest_recipients(&state.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "digest: could not list recipients");
            return;
        }
    };
    let now = Utc::now().to_rfc3339();
    let mut sent = 0u32;
    for r in recipients {
        let items = store::unread_notifications_since(
            &state.db,
            r.user_id,
            r.last_digest_at.as_deref(),
        )
        .await
        .unwrap_or_default();
        if items.is_empty() {
            continue; // no digest when there is nothing pending
        }
        let body = render_digest(&items);
        if store::create_notification(&state.db, r.user_id, NotificationKind::Digest, None, &body)
            .await
            .is_ok()
        {
            state.bus.publish(BoardEvent::Notified { user_id: r.user_id });
            emit_bread(state, &BoardEvent::Notified { user_id: r.user_id });
            // Advance the window only on a real send, so every item lands in
            // exactly one digest.
            if let Err(e) = store::set_last_digest_at(&state.db, r.user_id, &now).await {
                tracing::warn!(user_id = r.user_id, error = %e, "digest: could not record last-sent");
            }
            sent += 1;
        }
    }
    if sent > 0 {
        tracing::info!(users = sent, "daily digest sent");
    }
}

/// Render the digest body: a count line plus the first 20 items.
fn render_digest(items: &[Notification]) -> String {
    let n = items.len();
    let plural = if n == 1 { "" } else { "s" };
    let mut s = format!("Daily digest: {n} unread update{plural}\n");
    for it in items.iter().take(20) {
        s.push_str("\n- ");
        s.push_str(&it.body);
    }
    if n > 20 {
        s.push_str(&format!("\n- and {} more", n - 20));
    }
    s
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
