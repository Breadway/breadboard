//! The Forgejo sync engine: apply inbound changes (webhook + reconcile),
//! push breadboard changes back, and run the periodic reconcile loop.
//!
//! Conflict model: per-field, the side with the newer issue-level `updated_at`
//! wins, and a value that already matches is a no-op (this absorbs the webhook
//! echo of our own writeback). Every applied change is logged to `sync_log` by
//! the store layer; when breadboard is the newer side and the two still differ,
//! a `sync.drift` event is emitted.

use anyhow::Context;
use breadboard_core::{BoardEvent, Issue, Status, SyncSource};
use chrono::{DateTime, Utc};

use crate::dto::IssuePatch;
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
        // New to breadboard — import it.
        let project = match store::get_project_by_repo(&state.db, repo).await {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(%repo, "synced repo has no mapped project; skipping issue import");
                return Ok(());
            }
        };
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

    match state
        .forgejo
        .create_issue(owner, name, &issue.title, &issue.body, &issue.labels, &assignees)
        .await
    {
        Ok(fg) => match store::link_issue(&state.db, issue.id, &repo, fg.number).await {
            Ok(linked) => {
                tracing::info!(key = %linked.key, number = fg.number, %repo, "created on forgejo");
                linked
            }
            Err(e) => {
                tracing::warn!(key = %issue.key, error = %e, "linking new forgejo issue failed");
                issue
            }
        },
        Err(e) => {
            tracing::warn!(key = %issue.key, error = %e, "forgejo issue creation failed");
            issue
        }
    }
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
