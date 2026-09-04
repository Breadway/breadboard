//! Inbound Forgejo webhook: `POST /webhooks/forgejo`.
//!
//! Verified by HMAC-SHA256 over the raw body (`forgejo.webhook_secret`), so it
//! needs no session/token auth. Unknown or disabled repos return `200` (nothing
//! to do) so Forgejo doesn't retry.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::forgejo::{FgComment, FgIssue};
use crate::{store, sync, AppState};

type HmacSha256 = Hmac<Sha256>;

#[derive(Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(Deserialize)]
struct IssuePayload {
    #[serde(default)]
    action: String,
    issue: FgIssue,
    repository: Repository,
}

#[derive(Deserialize)]
struct CommentPayload {
    #[serde(default)]
    action: String,
    issue: FgIssue,
    comment: FgComment,
    repository: Repository,
}

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let secret = &state.config.forgejo.webhook_secret;
    if secret.is_empty() {
        tracing::warn!("webhook received but forgejo.webhook_secret is unset");
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if !verify(secret, &headers, &body) {
        return StatusCode::UNAUTHORIZED;
    }

    let event = headers
        .get("x-forgejo-event")
        .or_else(|| headers.get("x-gitea-event"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let result = match event.as_str() {
        "issues" => match serde_json::from_slice::<IssuePayload>(&body) {
            Ok(p) => apply_issue(&state, p).await,
            Err(e) => Err(e.into()),
        },
        "issue_comment" => match serde_json::from_slice::<CommentPayload>(&body) {
            Ok(p) => apply_comment(&state, p).await,
            Err(e) => Err(e.into()),
        },
        "pull_request" => {
            // Mirrored read-mostly; full PR sync lands in a follow-up.
            tracing::debug!("ignoring pull_request webhook (not yet synced)");
            Ok(())
        }
        other => {
            tracing::debug!(event = other, "ignoring unhandled webhook event");
            Ok(())
        }
    };

    if let Err(e) = result {
        tracing::warn!(event = %event, error = %e, "webhook handling failed");
    }
    // Always 200 once authenticated — the reconcile pass is the safety net.
    StatusCode::OK
}

async fn apply_issue(state: &AppState, p: IssuePayload) -> anyhow::Result<()> {
    if !store::repo_enabled(&state.db, &p.repository.full_name).await? {
        return Ok(());
    }
    tracing::info!(repo = %p.repository.full_name, number = p.issue.number, action = %p.action, "webhook: issue");
    sync::apply_issue(
        state,
        &p.repository.full_name,
        &p.issue,
        breadboard_core::SyncSource::ForgejoWebhook,
    )
    .await
}

async fn apply_comment(state: &AppState, p: CommentPayload) -> anyhow::Result<()> {
    if !store::repo_enabled(&state.db, &p.repository.full_name).await? {
        return Ok(());
    }
    if p.action == "deleted" {
        // Comment deletes are rare; the reconcile pass doesn't cover them yet.
        return Ok(());
    }
    // Make sure the issue exists locally first (comment may arrive before we've
    // imported a brand-new issue).
    sync::apply_issue(
        state,
        &p.repository.full_name,
        &p.issue,
        breadboard_core::SyncSource::ForgejoWebhook,
    )
    .await?;
    sync::apply_comment(state, &p.repository.full_name, p.issue.number, &p.comment).await
}

fn verify(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let sig = headers
        .get("x-forgejo-signature")
        .or_else(|| headers.get("x-gitea-signature"))
        .and_then(|v| v.to_str().ok());
    let Some(sig) = sig else {
        tracing::warn!("webhook missing signature header");
        return false;
    };
    let Ok(sig_bytes) = hex::decode(sig.trim()) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key size");
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}
