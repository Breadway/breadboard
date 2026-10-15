//! Import a markdown outline into epics + issues.
//!
//! `##` / `#` headings become epics; `- [ ]` / `- [x]` checklist items under a
//! heading become issues in the given project (checked → `done`). Plain list
//! items and prose are ignored. Idempotent-ish: an epic/issue whose title
//! already exists in the project is skipped.

use breadboard_core::{Status, SyncSource};
use serde::{Deserialize, Serialize};

use crate::dto::{NewEpic, NewIssue};
use crate::error::{AppError, AppResult};
use crate::{store, AppState};

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    /// Project key to file everything under.
    pub project: String,
    pub markdown: String,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportResult {
    pub epics_created: Vec<String>,
    pub issues_created: Vec<String>,
    pub skipped: Vec<String>,
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

    let parsed = parse(&req.markdown);
    let existing_issues: std::collections::HashSet<String> = store::list_issues(
        &state.db,
        &crate::dto::IssueFilter {
            project: Some(project.key.clone()),
            ..Default::default()
        },
    )
    .await?
    .into_iter()
    .map(|i| i.title.to_lowercase())
    .collect();
    let existing_epics: std::collections::HashSet<String> = store::list_epics(&state.db)
        .await?
        .into_iter()
        .map(|e| e.title.to_lowercase())
        .collect();

    let mut result = ImportResult::default();
    let mut ep_seq = 1;

    for pe in parsed {
        let epic_id = if existing_epics.contains(&pe.title.to_lowercase()) {
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
            if existing_issues.contains(&title.to_lowercase()) {
                result.skipped.push(format!("issue: {title}"));
                continue;
            }
            if req.dry_run {
                result.issues_created.push(title);
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
            result.issues_created.push(issue.key);
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
