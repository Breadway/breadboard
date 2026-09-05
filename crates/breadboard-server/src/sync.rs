//! The Forgejo sync engine: apply inbound changes (webhook + reconcile),
//! push breadboard changes back, and run the periodic reconcile loop.
//!
//! Conflict model: per-field, the side with the newer issue-level `updated_at`
//! wins, and a value that already matches is a no-op (this absorbs the webhook
//! echo of our own writeback). Every applied change is logged to `sync_log` by
//! the store layer; when breadboard is the newer side and the two still differ,
//! a `sync.drift` event is emitted.

use anyhow::Context;
use breadboard_core::{BoardEvent, Issue, IssueType, LinkKind, Project, Status, SyncSource};
use chrono::{DateTime, Utc};

use crate::dto::IssuePatch;
use crate::error::AppResult;
use crate::forgejo::{split_repo, FgComment, FgIssue, FgIssuePatch, FgUser};
use crate::{store, AppState};

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// Forgejo only tracks open/closed; breadboard's finer statuses survive except
/// across the done boundary. `pub` for tests.
pub fn map_status(fg_closed: bool, current: Status) -> Status {
    match (fg_closed, current) {
        (true, s) if s != Status::Done => Status::Done,
        (false, Status::Done) => Status::Todo,
        (_, s) => s,
    }
}

fn norm_body(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

const CLOSE_KEYWORDS: &[&str] = &[
    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
];

/// Issue numbers referenced by a closing keyword ("closes #4", "Fixes #4, #5")
/// in a PR body — the same convention Forgejo/GitHub use for auto-closing.
fn parse_closes_refs(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let start = i;
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                j += 1;
            }
            if CLOSE_KEYWORDS.contains(&&lower[start..j]) {
                let mut k = j;
                loop {
                    while k < bytes.len() && matches!(bytes[k], b' ' | b',' | b':') {
                        k += 1;
                    }
                    if lower[k..].starts_with("and ") {
                        k += 4;
                        continue;
                    }
                    if k < bytes.len() && bytes[k] == b'#' {
                        let num_start = k + 1;
                        let mut m = num_start;
                        while m < bytes.len() && bytes[m].is_ascii_digit() {
                            m += 1;
                        }
                        if m > num_start {
                            if let Ok(n) = lower[num_start..m].parse::<i64>() {
                                if !out.contains(&n) {
                                    out.push(n);
                                }
                            }
                            k = m;
                            continue;
                        }
                    }
                    break;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Sync a PR's "closes" relationships: link it to the issue(s) its body
/// references (so the board shows the connection instead of two
/// seemingly-unrelated cards), and — since Forgejo doesn't reliably
/// auto-close every issue in a comma-separated "Closes #a, #b" list
/// (confirmed: only the first one actually closes) — cascade breadboard's
/// own `done` status onto all of them once the PR itself is merged/closed,
/// best-effort closing their Forgejo issues too. Idempotent (`add_link` is
/// `INSERT OR IGNORE`, and the status cascade no-ops once already done) —
/// safe to call on every sync of a PR, not just its creation.
async fn sync_pr_closes(state: &AppState, repo: &str, pr_issue_id: i64, body: &str, pr_done: bool) {
    for number in parse_closes_refs(body) {
        let Ok(Some(target)) = store::find_issue_by_forgejo(&state.db, repo, number).await else {
            continue;
        };
        if target.id == pr_issue_id {
            continue;
        }
        let _ = store::add_link(&state.db, pr_issue_id, target.id, LinkKind::Closes).await;
        let _ = store::add_link(&state.db, target.id, pr_issue_id, LinkKind::ClosedBy).await;

        if !pr_done || target.status == Status::Done {
            continue;
        }
        let patch = IssuePatch {
            status: Some(Status::Done),
            ..Default::default()
        };
        let Ok(updated) =
            store::update_issue(&state.db, &target.key, &patch, None, SyncSource::ForgejoWebhook)
                .await
        else {
            continue;
        };
        emit_change_events(state, &target, &updated);
        let Some(fref) = &updated.forgejo_ref else { continue };
        if !state.forgejo.is_configured() {
            continue;
        }
        if let Ok((owner, r)) = split_repo(&fref.repo) {
            if let Err(e) = state
                .forgejo
                .patch_issue(
                    owner,
                    r,
                    fref.number,
                    FgIssuePatch {
                        state: Some("closed"),
                        ..Default::default()
                    },
                )
                .await
            {
                tracing::warn!(key = %updated.key, error = %e, "closing cascaded issue on forgejo failed");
            }
        }
    }
}

async fn resolve_user(state: &AppState, fg: Option<&FgUser>) -> anyhow::Result<Option<i64>> {
    let Some(u) = fg else { return Ok(None) };
    let display = if u.full_name.trim().is_empty() {
        u.login.clone()
    } else {
        u.full_name.clone()
    };
    let user = store::upsert_user(&state.db, u.id, &u.login, &display, u.avatar_url.as_deref())
        .await
        .context("upserting synced user")?;
    Ok(Some(user.id))
}

/// Apply one Forgejo issue (from a webhook or the reconcile pass).
pub async fn apply_issue(
    state: &AppState,
    repo: &str,
    fg: &FgIssue,
    source: SyncSource,
) -> anyhow::Result<()> {
    let existing = store::find_issue_by_forgejo(&state.db, repo, fg.number).await?;
    let assignee_id = resolve_user(state, fg.first_assignee()).await?;
    let labels = fg.label_names();

    let Some(before) = existing else {
        // New to breadboard — but first check whether this is the echo of an
        // issue breadboard itself just created (writeback_new_issue's own
        // link_issue call can lose this race to the webhook). If so, claim
        // that pending local issue instead of importing a duplicate.
        let project = match store::get_project_by_repo(&state.db, repo).await {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(%repo, "synced repo has no mapped project; skipping issue import");
                return Ok(());
            }
        };
        if let Some(claimed) =
            store::claim_unlinked_issue(&state.db, project.id, fg.title.trim(), repo, fg.number)
                .await?
        {
            tracing::info!(key = %claimed.key, number = fg.number, %repo, "claimed pending local issue instead of duplicating");
            return Ok(());
        }
        let status = map_status(fg.is_closed(), Status::Backlog);
        let issue = store::import_issue(
            &state.db,
            &project,
            repo,
            fg.number,
            fg.title.trim(),
            &norm_body(&fg.body),
            status,
            fg.is_pr(),
            &labels,
            assignee_id,
            fg.milestone.as_ref().map(|m| m.title.as_str()),
            &fg.created_at,
            &fg.updated_at,
            source,
        )
        .await?;
        if fg.is_pr() {
            sync_pr_closes(state, repo, issue.id, &fg.body, status == Status::Done).await;
        }
        state.bus.publish(BoardEvent::CardCreated {
            id: issue.id,
            key: issue.key,
        });
        return Ok(());
    };

    let fg_newer = match (parse_ts(&fg.updated_at), Some(before.updated_at)) {
        (Some(fg_t), Some(bb_t)) => fg_t >= bb_t,
        _ => true,
    };

    // Diff the mapped fields.
    let mut patch = IssuePatch::default();
    let mut drift: Vec<&'static str> = Vec::new();
    let target_status = map_status(fg.is_closed(), before.status);

    macro_rules! field {
        ($name:literal, $differs:expr, $apply:block) => {
            if $differs {
                if fg_newer {
                    $apply
                } else {
                    drift.push($name);
                }
            }
        };
    }

    field!("title", fg.title.trim() != before.title, {
        patch.title = Some(fg.title.trim().to_string());
    });
    field!("body", norm_body(&fg.body) != norm_body(&before.body), {
        patch.body = Some(norm_body(&fg.body));
    });
    field!("status", target_status != before.status, {
        patch.status = Some(target_status);
    });
    field!("assignee_id", assignee_id != before.assignee_id, {
        patch.assignee_id = Some(assignee_id);
    });
    field!("labels", !same_set(&labels, &before.labels), {
        patch.labels = Some(labels.clone());
    });
    let fg_milestone = fg.milestone.as_ref().map(|m| m.title.clone());
    field!("milestone", fg_milestone != before.milestone, {
        patch.milestone = Some(fg_milestone.clone());
    });

    for field in drift {
        state.bus.publish(BoardEvent::SyncDrift {
            issue_id: before.id,
            key: before.key.clone(),
            field: field.to_string(),
        });
    }

    if patch.is_empty() {
        return Ok(());
    }

    let after = store::update_issue(&state.db, &before.key, &patch, None, source).await?;
    emit_change_events(state, &before, &after);
    if after.assignee_id != before.assignee_id {
        if let Some(a) = after.assignee_id {
            crate::notify::assigned(state, &after, a, None).await;
        }
    }
    if after.issue_type == IssueType::Pr {
        sync_pr_closes(state, repo, after.id, &after.body, after.status == Status::Done).await;
    }
    Ok(())
}

/// Apply one Forgejo comment.
pub async fn apply_comment(
    state: &AppState,
    repo: &str,
    issue_number: i64,
    fg: &FgComment,
) -> anyhow::Result<()> {
    let Some(issue) = store::find_issue_by_forgejo(&state.db, repo, issue_number).await? else {
        return Ok(());
    };
    let author_id = resolve_user(state, fg.user.as_ref()).await?;
    store::upsert_synced_comment(
        &state.db,
        issue.id,
        fg.id,
        author_id,
        &fg.body,
        &fg.created_at,
        &fg.updated_at,
    )
    .await?;
    state.bus.publish(BoardEvent::CardUpdated {
        id: issue.id,
        key: issue.key,
    });
    Ok(())
}

/// Push a breadboard change back to Forgejo. Called after a successful PATCH.
/// The webhook echo this generates is absorbed by the equality check in
/// [`apply_issue`].
pub async fn writeback(state: &AppState, before: &Issue, after: &Issue) {
    let Some(fref) = &after.forgejo_ref else { return };
    if !state.forgejo.is_configured() {
        return;
    }
    match store::repo_enabled(&state.db, &fref.repo).await {
        Ok(true) => {}
        _ => return,
    }
    let Ok((owner, repo)) = split_repo(&fref.repo) else { return };

    // Labels round-trip for both issues and PRs; the rest is issues-only
    // (PRs are mirrored read-mostly).
    if !same_set(&after.labels, &before.labels) {
        if let Err(e) = state
            .forgejo
            .replace_labels(owner, repo, fref.number, &after.labels)
            .await
        {
            tracing::warn!(key = %after.key, error = %e, "forgejo writeback (labels) failed");
        }
    }
    if after.issue_type == breadboard_core::IssueType::Pr {
        return;
    }

    let title = (after.title != before.title).then_some(after.title.as_str());
    let body = (norm_body(&after.body) != norm_body(&before.body)).then(|| norm_body(&after.body));
    let state_change = if after.status != before.status
        && after.status.is_closed() != before.status.is_closed()
    {
        Some(if after.status.is_closed() { "closed" } else { "open" })
    } else {
        None
    };
    let assignees: Option<Vec<String>> = if after.assignee_id != before.assignee_id {
        match after.assignee_id {
            Some(id) => match store::get_user(&state.db, id).await {
                Ok(u) => Some(vec![u.login]),
                Err(_) => Some(vec![]),
            },
            None => Some(vec![]),
        }
    } else {
        None
    };

    let fg_patch = FgIssuePatch {
        title,
        body: body.as_deref(),
        state: state_change,
        assignees: assignees.as_deref(),
    };
    if !fg_patch.is_empty() {
        if let Err(e) = state
            .forgejo
            .patch_issue(owner, repo, fref.number, fg_patch)
            .await
        {
            tracing::warn!(key = %after.key, error = %e, "forgejo writeback (patch) failed");
        }
    }
}

/// If a newly-created breadboard issue belongs to a project mapped to an enabled
/// repo, create it on Forgejo too and link it. Returns the linked issue (or the
/// original if nothing was pushed). Best-effort — logs on failure.
pub async fn writeback_new_issue(state: &AppState, issue: Issue) -> Issue {
    if issue.forgejo_ref.is_some() || !state.forgejo.is_configured() {
        return issue;
    }
    let Ok(project) = store::get_project(&state.db, issue.project_id).await else {
        return issue;
    };
    let Some(repo) = project.forgejo_repo else { return issue };
    if !matches!(store::repo_enabled(&state.db, &repo).await, Ok(true)) {
        return issue;
    }
    let Ok((owner, name)) = split_repo(&repo) else { return issue };

    let assignees: Vec<String> = match issue.assignee_id {
        Some(id) => match store::get_user(&state.db, id).await {
            Ok(u) => vec![u.login],
            Err(_) => vec![],
        },
        None => vec![],
    };

    let fg = match state
        .forgejo
        .create_issue(owner, name, &issue.title, &issue.body, &assignees)
        .await
    {
        Ok(fg) => fg,
        Err(e) => {
            tracing::warn!(key = %issue.key, error = %e, "forgejo issue creation failed");
            return issue;
        }
    };

    if !issue.labels.is_empty() {
        if let Err(e) = state
            .forgejo
            .replace_labels(owner, name, fg.number, &issue.labels)
            .await
        {
            tracing::warn!(key = %issue.key, error = %e, "setting labels on new forgejo issue failed");
        }
    }

    match store::link_issue(&state.db, issue.id, &repo, fg.number).await {
        Ok(linked) => {
            tracing::info!(key = %linked.key, number = fg.number, %repo, "created on forgejo");
            linked
        }
        Err(e) => {
            tracing::warn!(key = %issue.key, error = %e, "linking new forgejo issue failed");
            issue
        }
    }
}

/// Move an issue to a different project. Best-effort closes the old Forgejo
/// issue (it belongs to the old project's repo, if any) with a note pointing
/// at the new key, then re-creates it fresh on the new project's repo via
/// `writeback_new_issue` if one is mapped.
pub async fn move_project(
    state: &AppState,
    key: &str,
    new_project: &Project,
) -> AppResult<Issue> {
    let (issue, old_ref) = store::move_issue_project(&state.db, key, new_project).await?;

    if let Some(old) = old_ref {
        if state.forgejo.is_configured() {
            if let Ok((owner, repo)) = split_repo(&old.repo) {
                let _ = state
                    .forgejo
                    .create_comment(
                        owner,
                        repo,
                        old.number,
                        &format!("Moved to {} — closing here.", issue.key),
                    )
                    .await;
                if let Err(e) = state
                    .forgejo
                    .patch_issue(
                        owner,
                        repo,
                        old.number,
                        FgIssuePatch {
                            state: Some("closed"),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(key = %issue.key, error = %e, "closing old forgejo issue after project move failed");
                }
            }
        }
    }

    Ok(writeback_new_issue(state, issue).await)
}

/// Push a breadboard-authored comment to Forgejo, recording the returned id so
/// the webhook echo is deduped. Best-effort — logs on failure.
pub async fn writeback_comment(state: &AppState, issue: &Issue, comment_id: i64, body: &str) {
    let Some(fref) = &issue.forgejo_ref else { return };
    if !state.forgejo.is_configured() {
        return;
    }
    if !matches!(store::repo_enabled(&state.db, &fref.repo).await, Ok(true)) {
        return;
    }
    let Ok((owner, repo)) = split_repo(&fref.repo) else { return };
    match state.forgejo.create_comment(owner, repo, fref.number, body).await {
        Ok(created) => {
            store::set_comment_external_id(&state.db, comment_id, created.id)
                .await
                .ok();
        }
        Err(e) => tracing::warn!(key = %issue.key, error = %e, "forgejo comment writeback failed"),
    }
}

/// One reconcile pass over every enabled repo.
pub async fn reconcile_once(state: &AppState) -> anyhow::Result<()> {
    if !state.forgejo.is_configured() {
        return Ok(());
    }
    let repos = store::enabled_repos(&state.db).await?;
    for (repo, last_synced) in repos {
        let started = Utc::now().to_rfc3339();
        let (owner, name) = match split_repo(&repo) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%repo, error = %e, "bad repo name in repo_sync_state");
                continue;
            }
        };
        match state
            .forgejo
            .issues_since(owner, name, last_synced.as_deref())
            .await
        {
            Ok(issues) => {
                let count = issues.len();
                for fg in &issues {
                    if let Err(e) =
                        apply_issue(state, &repo, fg, SyncSource::ForgejoReconcile).await
                    {
                        tracing::warn!(%repo, number = fg.number, error = %e, "reconcile apply failed");
                    }
                }
                store::set_repo_synced_at(&state.db, &repo, &started).await.ok();
                tracing::info!(%repo, count, "reconcile pass complete");
            }
            Err(e) => tracing::warn!(%repo, error = %e, "reconcile fetch failed"),
        }
    }
    Ok(())
}

/// Spawn the background reconcile loop.
pub fn spawn_reconcile(state: AppState) {
    let interval_mins = state.config.forgejo.reconcile_interval_mins.max(1);
    tokio::spawn(async move {
        let mut tick =
            tokio::time::interval(std::time::Duration::from_secs(interval_mins * 60));
        loop {
            tick.tick().await;
            if let Err(e) = reconcile_once(&state).await {
                tracing::warn!(error = %e, "reconcile loop error");
            }
        }
    });
}

// --- helpers -------------------------------------------------------------

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&String> = a.iter().collect();
    let mut b: Vec<&String> = b.iter().collect();
    a.sort();
    b.sort();
    a == b
}

fn emit_change_events(state: &AppState, before: &Issue, after: &Issue) {
    if after.status != before.status {
        state.bus.publish(BoardEvent::CardMoved {
            id: after.id,
            key: after.key.clone(),
            from: before.status,
            to: after.status,
        });
    }
    if after.assignee_id != before.assignee_id {
        state.bus.publish(BoardEvent::CardAssigned {
            id: after.id,
            key: after.key.clone(),
            assignee_id: after.assignee_id,
        });
    }
    state.bus.publish(BoardEvent::CardUpdated {
        id: after.id,
        key: after.key.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::parse_closes_refs;

    #[test]
    fn closes_ref_parsing() {
        assert_eq!(parse_closes_refs("Closes #2, #3, #4"), vec![2, 3, 4]);
        assert_eq!(parse_closes_refs("fixes #5 and #6"), vec![5, 6]);
        assert_eq!(parse_closes_refs("Resolved: #7"), vec![7]);
        assert_eq!(
            parse_closes_refs("this discloses #8"),
            Vec::<i64>::new(),
            "discloses must not match closes as a substring"
        );
        assert_eq!(parse_closes_refs("no refs here"), Vec::<i64>::new());
        assert_eq!(parse_closes_refs("see #9 for context"), Vec::<i64>::new());
    }
}
