//! Import a markdown outline into epics + issues.
//!
//! `##` / `#` headings become epics; `- [ ]` / `- [x]` checklist items under a
//! heading become issues in the given project (checked → `done`). Plain list
//! items and prose are ignored. Idempotent-ish: an epic/issue whose title
//! already exists in the project is skipped.

use breadboard_core::{Status, SyncSource};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use crate::dto::{NewEpic, NewIssue};
use crate::error::{AppError, AppResult};
use crate::{store, AppState};

static IMPORT_LOCKS: OnceLock<std::sync::Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    /// Project key to file everything under.
    pub project: String,
    pub markdown: String,
    #[serde(default)]
    pub dry_run: bool,
    /// Repair existing local cards without a Forgejo link. Ordinary imports
    /// only mirror cards created by that request.
    #[serde(default)]
    pub repair: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportResult {
    pub epics_created: Vec<String>,
    pub issues_created: Vec<String>,
    pub skipped: Vec<String>,
    pub remote_would_create: Vec<String>,
    /// Issues that remain local because Forgejo creation or linking failed.
    pub pending_writebacks: Vec<PendingWriteback>,
}

#[derive(Debug, Serialize)]
pub struct PendingWriteback {
    pub key: String,
    pub error: String,
}

struct ParsedEpic {
    title: String,
    issues: Vec<(String, bool)>, // (title, done)
}

fn parse(md: &str) -> Vec<ParsedEpic> {
    let mut epics: Vec<ParsedEpic> = Vec::new();
    for raw in md.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('#') {
            let title = rest.trim_start_matches('#').trim();
            if !title.is_empty() {
                epics.push(ParsedEpic {
                    title: title.to_string(),
                    issues: Vec::new(),
                });
            }
            continue;
        }
        let checkbox = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("+ "));
        if let Some(item) = checkbox {
            let (done, text) = if let Some(t) = item
                .strip_prefix("[x]")
                .or_else(|| item.strip_prefix("[X]"))
            {
                (true, t.trim())
            } else if let Some(t) = item.strip_prefix("[ ]") {
                (false, t.trim())
            } else {
                continue; // a plain bullet, not a task
            };
            if text.is_empty() {
                continue;
            }
            if let Some(epic) = epics.last_mut() {
                epic.issues.push((strip_md(text), done));
            }
        }
    }
    epics.retain(|e| !e.issues.is_empty());
    epics
}

/// Strip inline markdown (links → their text, plus `**` and backticks).
fn strip_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' | '*' => {}
            '[' => {
                // [text](url): keep text, drop the rest of the link.
                let mut text = String::new();
                for ic in chars.by_ref() {
                    if ic == ']' {
                        break;
                    }
                    text.push(ic);
                }
                out.push_str(&text);
                if chars.peek() == Some(&'(') {
                    for ic in chars.by_ref() {
                        if ic == ')' {
                            break;
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

pub async fn run(state: &AppState, req: &ImportRequest) -> AppResult<ImportResult> {
    let project = store::get_project_by_key(&state.db, &req.project)
        .await
        .map_err(|_| AppError::bad_request(format!("unknown project `{}`", req.project)))?;
    let lock = IMPORT_LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(project.id)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    let parsed = parse(&req.markdown);
    let issue_list = store::list_issues(
        &state.db,
        &crate::dto::IssueFilter {
            project: Some(project.key.clone()),
            ..Default::default()
        },
    )
    .await?;
    let mut existing_issues: std::collections::HashMap<String, Vec<breadboard_core::Issue>> =
        HashMap::new();
    for issue in issue_list {
        existing_issues
            .entry(issue.title.to_lowercase())
            .or_default()
            .push(issue);
    }
    let mut titles = std::collections::HashSet::new();
    for epic in &parsed {
        for (title, _) in &epic.issues {
            if !titles.insert(title.to_lowercase()) {
                return Err(AppError::bad_request(format!(
                    "duplicate import item title `{title}` is ambiguous"
                )));
            }
        }
    }
    let mut existing_epics: std::collections::HashMap<String, Vec<i64>> = HashMap::new();
    for epic in store::list_epics(&state.db).await? {
        existing_epics
            .entry(epic.title.to_lowercase())
            .or_default()
            .push(epic.id);
    }

    let mut result = ImportResult::default();
    let mut ep_seq = 1;

    for pe in parsed {
        let epic_matches = existing_epics.get(&pe.title.to_lowercase());
        if epic_matches.is_some_and(|matches| matches.len() > 1) {
            return Err(AppError::bad_request(format!(
                "multiple existing epics match `{}`; refusing ambiguous import",
                pe.title
            )));
        }
        let epic_id = if epic_matches.is_some() {
            result.skipped.push(format!("epic: {}", pe.title));
            store::list_epics(&state.db)
                .await?
                .into_iter()
                .find(|e| e.title.eq_ignore_ascii_case(&pe.title))
                .map(|e| e.id)
        } else if req.dry_run {
            result.epics_created.push(pe.title.clone());
            None
        } else {
            let key = loop {
                let k = format!("{}-EP{ep_seq}", project.key);
                ep_seq += 1;
                if store::get_epic_by_key(&state.db, &k).await.is_err() {
                    break k;
                }
            };
            let epic = store::create_epic(
                &state.db,
                &NewEpic {
                    key,
                    title: pe.title.clone(),
                    description: None,
                    project_id: Some(project.id),
                },
            )
            .await?;
            result.epics_created.push(epic.key.clone());
            Some(epic.id)
        };

        for (title, done) in pe.issues {
            if let Some(matches) = existing_issues.get(&title.to_lowercase()) {
                if matches.len() > 1 {
                    return Err(AppError::bad_request(format!(
                        "multiple existing cards match `{title}`; refusing ambiguous import"
                    )));
                }
            }
            if let Some(existing) = existing_issues
                .get(&title.to_lowercase())
                .and_then(|v| (v.len() == 1).then(|| v[0].clone()))
            {
                result.skipped.push(format!("issue: {title}"));
                let repair_key = existing.key.clone();
                let is_unlinked = existing.forgejo_ref.is_none();
                if req.repair && !req.dry_run && existing.forgejo_ref.is_none() {
                    let (linked, error) =
                        crate::sync::writeback_new_issue_result(state, existing).await;
                    if let Some(error) = error {
                        result.pending_writebacks.push(PendingWriteback {
                            key: linked.key,
                            error,
                        });
                    } else if linked.forgejo_ref.is_some() {
                        existing_issues.insert(title.to_lowercase(), vec![linked]);
                    }
                }
                let repo_enabled = if let Some(repo) = &project.forgejo_repo {
                    matches!(store::repo_enabled(&state.db, repo).await, Ok(true))
                } else {
                    false
                };
                if req.repair && req.dry_run && is_unlinked && repo_enabled {
                    result.remote_would_create.push(repair_key);
                }
                continue;
            }
            if req.dry_run {
                result.issues_created.push(title.clone());
                let repo_enabled = if let Some(repo) = &project.forgejo_repo {
                    matches!(store::repo_enabled(&state.db, repo).await, Ok(true))
                } else {
                    false
                };
                if repo_enabled {
                    result.remote_would_create.push(title.clone());
                }
                continue;
            }
            let issue = store::create_issue(
                &state.db,
                &NewIssue {
                    title: title.clone(),
                    project: Some(project.key.clone()),
                    epic_id,
                    status: Some(if done { Status::Done } else { Status::Backlog }),
                    ..Default::default()
                },
                None,
            )
            .await?;
            // Log the provenance.
            sqlx::query(
                "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
                 VALUES (?1, 'created', NULL, 'markdown import', ?2, NULL, ?3)",
            )
            .bind(issue.id)
            .bind(SyncSource::Breadboard.as_str())
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&state.db)
            .await
            .ok();
            let (issue, error) = crate::sync::writeback_new_issue_result(state, issue).await;
            if let Some(error) = error {
                result.pending_writebacks.push(PendingWriteback {
                    key: issue.key.clone(),
                    error,
                });
            }
            result.issues_created.push(issue.key.clone());
            existing_issues.insert(title.to_lowercase(), vec![issue]);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_outline() {
        let md = "\
# Ecosystem platform pass
- [ ] pin bread-theme tag in **breadclip**
- [x] fix the [OSD bug](https://git.example.com/x) in breadbar
- a plain bullet, not a task

## Later
- [ ] `breadbox` sync flakiness
";
        let epics = parse(md);
        assert_eq!(epics.len(), 2);
        assert_eq!(epics[0].title, "Ecosystem platform pass");
        assert_eq!(epics[0].issues.len(), 2);
        assert_eq!(
            epics[0].issues[0],
            ("pin bread-theme tag in breadclip".into(), false)
        );
        assert_eq!(epics[0].issues[1].0, "fix the OSD bug in breadbar");
        assert!(epics[0].issues[1].1);
        assert_eq!(epics[1].issues[0].0, "breadbox sync flakiness");
    }
}
