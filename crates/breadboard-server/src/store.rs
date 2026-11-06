//! Database access + row→domain mapping.
//!
//! Runtime-checked `sqlx` queries (no compile-time macro, so the workspace
//! builds without a live database). Enums are stored as the snake/kebab-case
//! strings from `breadboard-core`; timestamps as RFC3339 UTC text.

use anyhow::Context;
use breadboard_core::{
    Comment, Cycle, Epic, ForgejoRef, Issue, IssueLink, IssueType, LinkKind, Priority, Project,
    Status, SyncSource, User,
};
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::dto::{IssueFilter, IssuePatch, NewCycle, NewEpic, NewIssue, NewProject};
use crate::error::{AppError, AppResult};

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// ---------------------------------------------------------------------------
// users
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct UserRow {
    id: i64,
    forgejo_id: i64,
    login: String,
    display_name: String,
    avatar_url: Option<String>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: r.id,
            forgejo_id: r.forgejo_id,
            login: r.login,
            display_name: r.display_name,
            avatar_url: r.avatar_url,
        }
    }
}

pub async fn upsert_user(
    db: &SqlitePool,
    forgejo_id: i64,
    login: &str,
    display_name: &str,
    avatar_url: Option<&str>,
) -> AppResult<User> {
    let row: UserRow = sqlx::query_as(
        r#"
        INSERT INTO users (forgejo_id, login, display_name, avatar_url)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(forgejo_id) DO UPDATE SET
            login = excluded.login,
            display_name = excluded.display_name,
            avatar_url = excluded.avatar_url
        RETURNING id, forgejo_id, login, display_name, avatar_url
        "#,
    )
    .bind(forgejo_id)
    .bind(login)
    .bind(display_name)
    .bind(avatar_url)
    .fetch_one(db)
    .await?;
    Ok(row.into())
}

pub async fn get_user(db: &SqlitePool, id: i64) -> AppResult<User> {
    let row: UserRow = sqlx::query_as(
        "SELECT id, forgejo_id, login, display_name, avatar_url FROM users WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

// ---------------------------------------------------------------------------
// projects
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct ProjectRow {
    id: i64,
    key: String,
    name: String,
    forgejo_repo: Option<String>,
    next_number: i64,
    archived: i64,
}

impl From<ProjectRow> for Project {
    fn from(r: ProjectRow) -> Self {
        Project {
            id: r.id,
            key: r.key,
            name: r.name,
            forgejo_repo: r.forgejo_repo,
            next_number: r.next_number,
            archived: r.archived != 0,
        }
    }
}

const PROJECT_COLS: &str = "id, key, name, forgejo_repo, next_number, archived";

pub async fn list_projects(db: &SqlitePool) -> AppResult<Vec<Project>> {
    let rows: Vec<ProjectRow> =
        sqlx::query_as(&format!("SELECT {PROJECT_COLS} FROM projects ORDER BY key"))
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_project(db: &SqlitePool, id: i64) -> AppResult<Project> {
    let row: ProjectRow =
        sqlx::query_as(&format!("SELECT {PROJECT_COLS} FROM projects WHERE id = ?1"))
            .bind(id)
            .fetch_optional(db)
            .await?
            .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

pub async fn get_project_by_key(db: &SqlitePool, key: &str) -> AppResult<Project> {
    let row: ProjectRow = sqlx::query_as(&format!(
        "SELECT {PROJECT_COLS} FROM projects WHERE key = ?1"
    ))
    .bind(key)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

/// Resolve a project by numeric id or by key, same convention as
/// `NewIssue::project`.
pub async fn find_project(db: &SqlitePool, s: &str) -> AppResult<Project> {
    if let Ok(id) = s.parse::<i64>() {
        if let Ok(p) = get_project(db, id).await {
            return Ok(p);
        }
    }
    get_project_by_key(db, s).await
}

pub async fn get_project_by_repo(db: &SqlitePool, repo: &str) -> AppResult<Project> {
    let row: ProjectRow = sqlx::query_as(&format!(
        "SELECT {PROJECT_COLS} FROM projects WHERE forgejo_repo = ?1"
    ))
    .bind(repo)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

/// Create the project if its key is free, or attach `forgejo_repo` to the
/// existing one. Used by `POST /api/repos`.
pub async fn upsert_project_for_repo(
    db: &SqlitePool,
    key: &str,
    name: &str,
    repo: &str,
) -> AppResult<Project> {
    if let Ok(p) = get_project_by_key(db, key).await {
        if p.forgejo_repo.as_deref() != Some(repo) {
            sqlx::query("UPDATE projects SET forgejo_repo = ?2 WHERE id = ?1")
                .bind(p.id)
                .bind(repo)
                .execute(db)
                .await?;
        }
        return get_project(db, p.id).await;
    }
    create_project(
        db,
        &NewProject {
            key: key.to_string(),
            name: name.to_string(),
            forgejo_repo: Some(repo.to_string()),
        },
    )
    .await
}

pub async fn create_project(db: &SqlitePool, new: &NewProject) -> AppResult<Project> {
    new.validate().map_err(AppError::BadRequest)?;
    let row: ProjectRow = sqlx::query_as(&format!(
        "INSERT INTO projects (key, name, forgejo_repo) VALUES (?1, ?2, ?3) RETURNING {PROJECT_COLS}"
    ))
    .bind(&new.key)
    .bind(&new.name)
    .bind(&new.forgejo_repo)
    .fetch_one(db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
            AppError::Conflict(format!("project key `{}` or repo already exists", new.key))
        }
        other => AppError::Db(other),
    })?;
    Ok(row.into())
}

pub async fn update_project(
    db: &SqlitePool,
    id: i64,
    patch: &crate::dto::ProjectPatch,
) -> AppResult<Project> {
    if patch.is_empty() {
        return get_project(db, id).await;
    }
    if let Some(name) = &patch.name {
        if name.trim().is_empty() {
            return Err(AppError::bad_request("name must not be empty"));
        }
        sqlx::query("UPDATE projects SET name = ?2 WHERE id = ?1")
            .bind(id)
            .bind(name.trim())
            .execute(db)
            .await?;
    }
    if let Some(archived) = patch.archived {
        sqlx::query("UPDATE projects SET archived = ?2 WHERE id = ?1")
            .bind(id)
            .bind(archived as i64)
            .execute(db)
            .await?;
    }
    if let Some(repo) = &patch.forgejo_repo {
        sqlx::query("UPDATE projects SET forgejo_repo = ?2 WHERE id = ?1")
            .bind(id)
            .bind(repo.as_deref())
            .execute(db)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    AppError::Conflict("that repo is already mapped to another project".into())
                }
                other => AppError::Db(other),
            })?;
    }
    get_project(db, id).await
}

// ---------------------------------------------------------------------------
// epics
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct EpicRow {
    id: i64,
    key: String,
    title: String,
    description: Option<String>,
    project_id: Option<i64>,
    archived: i64,
    created_at: String,
    updated_at: String,
}

impl From<EpicRow> for Epic {
    fn from(r: EpicRow) -> Self {
        Epic {
            id: r.id,
            key: r.key,
            title: r.title,
            description: r.description,
            project_id: r.project_id,
            archived: r.archived != 0,
            created_at: parse_ts(&r.created_at),
            updated_at: parse_ts(&r.updated_at),
        }
    }
}

const EPIC_COLS: &str =
    "id, key, title, description, project_id, archived, created_at, updated_at";

pub async fn list_epics(db: &SqlitePool) -> AppResult<Vec<Epic>> {
    let rows: Vec<EpicRow> =
        sqlx::query_as(&format!("SELECT {EPIC_COLS} FROM epics ORDER BY key"))
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_epic(db: &SqlitePool, id: i64) -> AppResult<Epic> {
    let row: EpicRow = sqlx::query_as(&format!("SELECT {EPIC_COLS} FROM epics WHERE id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

pub async fn get_epic_by_key(db: &SqlitePool, key: &str) -> AppResult<Epic> {
    let row: EpicRow = sqlx::query_as(&format!("SELECT {EPIC_COLS} FROM epics WHERE key = ?1"))
        .bind(key)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

pub async fn create_epic(db: &SqlitePool, new: &NewEpic) -> AppResult<Epic> {
    if new.key.trim().is_empty() || new.title.trim().is_empty() {
        return Err(AppError::bad_request("key and title are required"));
    }
    let ts = now_rfc3339();
    let row: EpicRow = sqlx::query_as(&format!(
        "INSERT INTO epics (key, title, description, project_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5) RETURNING {EPIC_COLS}"
    ))
    .bind(&new.key)
    .bind(&new.title)
    .bind(&new.description)
    .bind(new.project_id)
    .bind(&ts)
    .fetch_one(db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
            AppError::Conflict(format!("epic key `{}` already exists", new.key))
        }
        other => AppError::Db(other),
    })?;
    Ok(row.into())
}

pub async fn update_epic(
    db: &SqlitePool,
    id: i64,
    patch: &crate::dto::EpicPatch,
) -> AppResult<Epic> {
    let mut did = false;
    if let Some(title) = &patch.title {
        if title.trim().is_empty() {
            return Err(AppError::bad_request("title must not be empty"));
        }
        sqlx::query("UPDATE epics SET title = ?2 WHERE id = ?1")
            .bind(id)
            .bind(title.trim())
            .execute(db)
            .await?;
        did = true;
    }
    if let Some(desc) = &patch.description {
        sqlx::query("UPDATE epics SET description = ?2 WHERE id = ?1")
            .bind(id)
            .bind(desc.as_deref())
            .execute(db)
            .await?;
        did = true;
    }
    if let Some(archived) = patch.archived {
        sqlx::query("UPDATE epics SET archived = ?2 WHERE id = ?1")
            .bind(id)
            .bind(archived as i64)
            .execute(db)
            .await?;
        did = true;
    }
    if did {
        sqlx::query("UPDATE epics SET updated_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(now_rfc3339())
            .execute(db)
            .await?;
    }
    get_epic(db, id).await
}

pub async fn delete_epic(db: &SqlitePool, id: i64) -> AppResult<()> {
    let n = sqlx::query("DELETE FROM epics WHERE id = ?1")
        .bind(id)
        .execute(db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Per-status child-issue counts for an epic (or, with `epic_id = None`, for
/// issues with no epic; the backlog).
pub async fn epic_progress(db: &SqlitePool, epic_id: i64) -> AppResult<[(String, i64); 5]> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM issues WHERE epic_id = ?1 GROUP BY status",
    )
    .bind(epic_id)
    .fetch_all(db)
    .await?;
    let count = |s: &str| rows.iter().find(|(st, _)| st == s).map(|(_, n)| *n).unwrap_or(0);
    Ok([
        ("backlog".into(), count("backlog")),
        ("todo".into(), count("todo")),
        ("doing".into(), count("doing")),
        ("review".into(), count("review")),
        ("done".into(), count("done")),
    ])
}

// ---------------------------------------------------------------------------
// cycles
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct CycleRow {
    id: i64,
    name: String,
    starts_on: String,
    ends_on: String,
    closed: i64,
}

impl From<CycleRow> for Cycle {
    fn from(r: CycleRow) -> Self {
        Cycle {
            id: r.id,
            name: r.name,
            starts_on: r.starts_on.parse().unwrap_or_default(),
            ends_on: r.ends_on.parse().unwrap_or_default(),
            closed: r.closed != 0,
        }
    }
}

const CYCLE_COLS: &str = "id, name, starts_on, ends_on, closed";

pub async fn list_cycles(db: &SqlitePool) -> AppResult<Vec<Cycle>> {
    let rows: Vec<CycleRow> =
        sqlx::query_as(&format!("SELECT {CYCLE_COLS} FROM cycles ORDER BY starts_on DESC"))
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_cycle(db: &SqlitePool, id: i64) -> AppResult<Cycle> {
    let row: CycleRow = sqlx::query_as(&format!("SELECT {CYCLE_COLS} FROM cycles WHERE id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(row.into())
}

pub async fn create_cycle(db: &SqlitePool, new: &NewCycle) -> AppResult<Cycle> {
    if new.name.trim().is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    if new.ends_on < new.starts_on {
        return Err(AppError::bad_request("ends_on must be on or after starts_on"));
    }
    let row: CycleRow = sqlx::query_as(&format!(
        "INSERT INTO cycles (name, starts_on, ends_on) VALUES (?1, ?2, ?3) RETURNING {CYCLE_COLS}"
    ))
    .bind(&new.name)
    .bind(new.starts_on.to_string())
    .bind(new.ends_on.to_string())
    .fetch_one(db)
    .await?;
    Ok(row.into())
}

pub async fn set_cycle_closed(db: &SqlitePool, id: i64, closed: bool) -> AppResult<Cycle> {
    let affected = sqlx::query("UPDATE cycles SET closed = ?2 WHERE id = ?1")
        .bind(id)
        .bind(closed as i64)
        .execute(db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound);
    }
    get_cycle(db, id).await
}

pub async fn count_cycle_issues(db: &SqlitePool, id: i64) -> AppResult<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issues WHERE cycle_id = ?1")
        .bind(id)
        .fetch_one(db)
        .await?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// issues
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct IssueRow {
    id: i64,
    key: String,
    project_id: i64,
    title: String,
    body: String,
    status: String,
    priority: String,
    #[sqlx(rename = "type")]
    issue_type: String,
    estimate: Option<i64>,
    assignee_id: Option<i64>,
    reporter_id: Option<i64>,
    epic_id: Option<i64>,
    cycle_id: Option<i64>,
    forgejo_repo: Option<String>,
    forgejo_number: Option<i64>,
    milestone: Option<String>,
    created_at: String,
    updated_at: String,
}

const ISSUE_COLS: &str = "id, key, project_id, title, body, status, priority, type, estimate, \
    assignee_id, reporter_id, epic_id, cycle_id, forgejo_repo, forgejo_number, milestone, \
    created_at, updated_at";

/// Fields whose changes are recorded in `sync_log`. Of these, `title` / `body` /
/// `status` / `assignee_id` / `labels` also drive Forgejo writeback;
/// `milestone` is inbound-only.
pub const SYNCED_FIELDS: &[&str] =
    &["title", "body", "status", "assignee_id", "labels", "milestone"];

async fn hydrate(db: &SqlitePool, r: IssueRow) -> AppResult<Issue> {
    let labels: Vec<String> =
        sqlx::query_scalar("SELECT label FROM issue_labels WHERE issue_id = ?1 ORDER BY label")
            .bind(r.id)
            .fetch_all(db)
            .await?;

    let link_rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT target_id, kind FROM issue_links WHERE issue_id = ?1")
            .bind(r.id)
            .fetch_all(db)
            .await?;
    let links = link_rows
        .into_iter()
        .filter_map(|(target_id, kind)| {
            LinkKind::parse(&kind).map(|kind| IssueLink { kind, target_id })
        })
        .collect();

    Ok(Issue {
        id: r.id,
        key: r.key,
        project_id: r.project_id,
        title: r.title,
        body: r.body,
        status: Status::parse(&r.status).unwrap_or(Status::Backlog),
        priority: Priority::parse(&r.priority).unwrap_or(Priority::None),
        issue_type: IssueType::parse(&r.issue_type).unwrap_or(IssueType::Issue),
        estimate: r.estimate,
        assignee_id: r.assignee_id,
        reporter_id: r.reporter_id,
        epic_id: r.epic_id,
        cycle_id: r.cycle_id,
        labels,
        links,
        forgejo_ref: match (r.forgejo_repo, r.forgejo_number) {
            (Some(repo), Some(number)) => Some(breadboard_core::ForgejoRef { repo, number }),
            _ => None,
        },
        milestone: r.milestone,
        created_at: parse_ts(&r.created_at),
        updated_at: parse_ts(&r.updated_at),
    })
}

pub async fn get_issue(db: &SqlitePool, id: i64) -> AppResult<Issue> {
    let row: IssueRow = sqlx::query_as(&format!("SELECT {ISSUE_COLS} FROM issues WHERE id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    hydrate(db, row).await
}

pub async fn get_issue_by_key(db: &SqlitePool, key: &str) -> AppResult<Issue> {
    let row: IssueRow =
        sqlx::query_as(&format!("SELECT {ISSUE_COLS} FROM issues WHERE key = ?1"))
            .bind(key)
            .fetch_optional(db)
            .await?
            .ok_or(AppError::NotFound)?;
    hydrate(db, row).await
}

pub async fn list_issues(db: &SqlitePool, filter: &IssueFilter) -> AppResult<Vec<Issue>> {
    let mut qb: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new(format!("SELECT {ISSUE_COLS} FROM issues WHERE 1=1"));

    if let Some(project) = &filter.project {
        // Accept a key or a numeric id.
        if let Ok(id) = project.parse::<i64>() {
            qb.push(" AND project_id = ").push_bind(id);
        } else {
            let pid = get_project_by_key(db, project).await?.id;
            qb.push(" AND project_id = ").push_bind(pid);
        }
    }
    if let Some(status) = filter.status {
        qb.push(" AND status = ").push_bind(status.as_str());
    }
    if let Some(cycle_id) = filter.cycle_id {
        qb.push(" AND cycle_id = ").push_bind(cycle_id);
    }
    if let Some(epic_id) = filter.epic_id {
        qb.push(" AND epic_id = ").push_bind(epic_id);
    }
    if let Some(assignee_id) = filter.assignee_id {
        qb.push(" AND assignee_id = ").push_bind(assignee_id);
    }
    qb.push(" ORDER BY updated_at DESC");

    let rows: Vec<IssueRow> = qb.build_query_as().fetch_all(db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(hydrate(db, r).await?);
    }
    Ok(out)
}

/// Create an issue. Only `title` is guaranteed; the key is allocated from the
/// project's counter inside a transaction.
pub async fn create_issue(
    db: &SqlitePool,
    new: &NewIssue,
    reporter_id: Option<i64>,
) -> AppResult<Issue> {
    new.validate().map_err(AppError::BadRequest)?;

    let mut tx = db.begin().await?;

    let project: ProjectRow = match &new.project {
        Some(p) if p.parse::<i64>().is_ok() => sqlx::query_as(&format!(
            "SELECT {PROJECT_COLS} FROM projects WHERE id = ?1"
        ))
        .bind(p.parse::<i64>().unwrap())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::bad_request("unknown project"))?,
        Some(p) => sqlx::query_as(&format!(
            "SELECT {PROJECT_COLS} FROM projects WHERE key = ?1"
        ))
        .bind(p)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::bad_request("unknown project"))?,
        // No silent default once there is a choice. Filing into whichever
        // project happens to be first is how unrelated work ends up in a
        // repo's tracker or in the catch-all roadmap project.
        None => {
            let candidates: Vec<ProjectRow> = sqlx::query_as(&format!(
                "SELECT {PROJECT_COLS} FROM projects WHERE archived = 0 ORDER BY key"
            ))
            .fetch_all(&mut *tx)
            .await?;
            match candidates.len() {
                // Unambiguous: the only project there is.
                1 => candidates.into_iter().next().unwrap(),
                0 => {
                    return Err(AppError::bad_request(
                        "no project exists yet: create one first (POST /api/projects). \
                         Use a repo-backed project for that repo's code, or a native \
                         project for work that has no repo yet.",
                    ))
                }
                _ => {
                    let keys: Vec<&str> = candidates.iter().map(|p| p.key.as_str()).collect();
                    return Err(AppError::bad_request(format!(
                        "project is required: file the issue where the work actually lives. \
                         Available: {}. Use a repo-backed project only for that repo's code; \
                         create a native project for local config, tooling, or a project not \
                         yet on Forgejo.",
                        keys.join(", ")
                    )));
                }
            }
        }
    };

    let number: i64 = project.next_number;
    sqlx::query("UPDATE projects SET next_number = next_number + 1 WHERE id = ?1")
        .bind(project.id)
        .execute(&mut *tx)
        .await?;
    let key = format!("{}-{}", project.key, number);
    let ts = now_rfc3339();

    let row: IssueRow = sqlx::query_as(&format!(
        "INSERT INTO issues
            (key, project_id, title, body, status, priority, type, estimate,
             assignee_id, reporter_id, epic_id, cycle_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'issue', ?7, ?8, ?9, ?10, ?11, ?12, ?12)
         RETURNING {ISSUE_COLS}"
    ))
    .bind(&key)
    .bind(project.id)
    .bind(new.title.trim())
    .bind(&new.body)
    .bind(new.status.unwrap_or(Status::Backlog).as_str())
    .bind(new.priority.unwrap_or(Priority::None).as_str())
    .bind(new.estimate)
    .bind(new.assignee_id)
    .bind(reporter_id)
    .bind(new.epic_id)
    .bind(new.cycle_id)
    .bind(&ts)
    .fetch_one(&mut *tx)
    .await?;

    for label in dedup(&new.labels) {
        sqlx::query("INSERT OR IGNORE INTO issue_labels (issue_id, label) VALUES (?1, ?2)")
            .bind(row.id)
            .bind(&label)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    hydrate(db, row).await
}

/// Apply a partial update. Writes a `sync_log` row per changed synced field.
pub async fn update_issue(
    db: &SqlitePool,
    key: &str,
    patch: &IssuePatch,
    actor_id: Option<i64>,
    source: SyncSource,
) -> AppResult<Issue> {
    let before = get_issue_by_key(db, key).await?;
    let mut tx = db.begin().await?;
    let mut changed: Vec<(&str, Option<String>, Option<String>)> = Vec::new();

    macro_rules! set_col {
        ($col:literal, $val:expr, $old:expr, $new:expr) => {{
            sqlx::query(concat!("UPDATE issues SET ", $col, " = ?2 WHERE id = ?1"))
                .bind(before.id)
                .bind($val)
                .execute(&mut *tx)
                .await?;
            changed.push(($col, $old, $new));
        }};
    }

    if let Some(title) = &patch.title {
        if title.trim().is_empty() {
            return Err(AppError::bad_request("title must not be empty"));
        }
        if *title != before.title {
            set_col!("title", title.trim(), Some(before.title.clone()), Some(title.clone()));
        }
    }
    if let Some(body) = &patch.body {
        if *body != before.body {
            set_col!("body", body.as_str(), Some(before.body.clone()), Some(body.clone()));
        }
    }
    if let Some(status) = patch.status {
        if status != before.status {
            set_col!(
                "status",
                status.as_str(),
                Some(before.status.as_str().to_string()),
                Some(status.as_str().to_string())
            );
        }
    }
    if let Some(priority) = patch.priority {
        if priority != before.priority {
            set_col!(
                "priority",
                priority.as_str(),
                Some(before.priority.as_str().to_string()),
                Some(priority.as_str().to_string())
            );
        }
    }
    if let Some(estimate) = patch.estimate {
        if estimate != before.estimate {
            set_col!(
                "estimate",
                estimate,
                before.estimate.map(|v| v.to_string()),
                estimate.map(|v| v.to_string())
            );
        }
    }
    if let Some(assignee_id) = patch.assignee_id {
        if assignee_id != before.assignee_id {
            set_col!(
                "assignee_id",
                assignee_id,
                before.assignee_id.map(|v| v.to_string()),
                assignee_id.map(|v| v.to_string())
            );
        }
    }
    if let Some(epic_id) = patch.epic_id {
        if epic_id != before.epic_id {
            set_col!(
                "epic_id",
                epic_id,
                before.epic_id.map(|v| v.to_string()),
                epic_id.map(|v| v.to_string())
            );
        }
    }
    if let Some(milestone) = &patch.milestone {
        if *milestone != before.milestone {
            set_col!(
                "milestone",
                milestone.clone(),
                before.milestone.clone(),
                milestone.clone()
            );
        }
    }
    if let Some(cycle_id) = patch.cycle_id {
        if cycle_id != before.cycle_id {
            set_col!(
                "cycle_id",
                cycle_id,
                before.cycle_id.map(|v| v.to_string()),
                cycle_id.map(|v| v.to_string())
            );
        }
    }

    if let Some(labels) = &patch.labels {
        let next = dedup(labels);
        if next != before.labels {
            sqlx::query("DELETE FROM issue_labels WHERE issue_id = ?1")
                .bind(before.id)
                .execute(&mut *tx)
                .await?;
            for label in &next {
                sqlx::query("INSERT INTO issue_labels (issue_id, label) VALUES (?1, ?2)")
                    .bind(before.id)
                    .bind(label)
                    .execute(&mut *tx)
                    .await?;
            }
            changed.push((
                "labels",
                Some(before.labels.join(",")),
                Some(next.join(",")),
            ));
        }
    }

    if let Some(links) = &patch.links {
        sqlx::query("DELETE FROM issue_links WHERE issue_id = ?1")
            .bind(before.id)
            .execute(&mut *tx)
            .await?;
        for link in links {
            sqlx::query(
                "INSERT OR IGNORE INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, ?3)",
            )
            .bind(before.id)
            .bind(link.target_id)
            .bind(link.kind.as_str())
            .execute(&mut *tx)
            .await?;
        }
    }

    if changed.is_empty() && patch.links.is_none() {
        return Ok(before);
    }

    let ts = now_rfc3339();
    sqlx::query("UPDATE issues SET updated_at = ?2 WHERE id = ?1")
        .bind(before.id)
        .bind(&ts)
        .execute(&mut *tx)
        .await?;

    for (field, old, new) in &changed {
        if SYNCED_FIELDS.contains(field) {
            sqlx::query(
                "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(before.id)
            .bind(field)
            .bind(old)
            .bind(new)
            .bind(source.as_str())
            .bind(actor_id)
            .bind(&ts)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    let after = get_issue_by_key(db, key).await?;
    if after.status != before.status {
        let _ = sqlx::query(
            "UPDATE agent_claims SET board_status = ?2, updated_at = ?3 WHERE issue_id = ?1",
        )
        .bind(after.id)
        .bind(after.status.as_str())
        .bind(&ts)
        .execute(db)
        .await;
    }
    Ok(after)
}

/// Move an issue to a different project: allocate a fresh key from the
/// target project's counter and clear any Forgejo link (it belonged to the
/// old project's repo). Returns the moved issue and the link it had before
/// the move, so the caller can close it out on Forgejo and re-sync into the
/// new project's repo if one is mapped.
pub async fn move_issue_project(
    db: &SqlitePool,
    key: &str,
    new_project: &Project,
) -> AppResult<(Issue, Option<ForgejoRef>)> {
    let before = get_issue_by_key(db, key).await?;
    if before.project_id == new_project.id {
        let old_ref = before.forgejo_ref.clone();
        return Ok((before, old_ref));
    }
    let old_ref = before.forgejo_ref.clone();

    let mut tx = db.begin().await?;
    let n: i64 = new_project.next_number;
    sqlx::query("UPDATE projects SET next_number = next_number + 1 WHERE id = ?1")
        .bind(new_project.id)
        .execute(&mut *tx)
        .await?;
    let new_key = format!("{}-{}", new_project.key, n);
    let ts = now_rfc3339();

    sqlx::query(
        "UPDATE issues SET project_id = ?2, key = ?3, forgejo_repo = NULL,
            forgejo_number = NULL, milestone = NULL, updated_at = ?4 WHERE id = ?1",
    )
    .bind(before.id)
    .bind(new_project.id)
    .bind(&new_key)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
         VALUES (?1, 'project', ?2, ?3, 'breadboard', NULL, ?4)",
    )
    .bind(before.id)
    .bind(&before.key)
    .bind(&new_key)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    let after = get_issue_by_key(db, &new_key).await?;
    Ok((after, old_ref))
}

/// Add a single link if it doesn't already exist, without touching the
/// issue's other links (unlike `IssuePatch.links`, which replaces the whole
/// set; this is for sync-driven links like `closes`/`closed-by`).
pub async fn add_link(
    db: &SqlitePool,
    issue_id: i64,
    target_id: i64,
    kind: LinkKind,
) -> AppResult<()> {
    sqlx::query("INSERT OR IGNORE INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, ?3)")
        .bind(issue_id)
        .bind(target_id)
        .bind(kind.as_str())
        .execute(db)
        .await?;
    Ok(())
}

pub async fn find_issue_by_forgejo(
    db: &SqlitePool,
    repo: &str,
    number: i64,
) -> AppResult<Option<Issue>> {
    let row: Option<IssueRow> = sqlx::query_as(&format!(
        "SELECT {ISSUE_COLS} FROM issues WHERE forgejo_repo = ?1 AND forgejo_number = ?2"
    ))
    .bind(repo)
    .bind(number)
    .fetch_optional(db)
    .await?;
    match row {
        Some(r) => Ok(Some(hydrate(db, r).await?)),
        None => Ok(None),
    }
}

/// Import a Forgejo issue/PR as a new breadboard issue: allocate a key from the
/// mapped project, set the `forgejo_ref`, and log the creation to `sync_log`.
#[allow(clippy::too_many_arguments)]
pub async fn import_issue(
    db: &SqlitePool,
    project: &Project,
    repo: &str,
    number: i64,
    title: &str,
    body: &str,
    status: Status,
    is_pr: bool,
    labels: &[String],
    assignee_id: Option<i64>,
    milestone: Option<&str>,
    created_at: &str,
    updated_at: &str,
    source: SyncSource,
) -> AppResult<Issue> {
    let mut tx = db.begin().await?;
    let n: i64 = project.next_number;
    sqlx::query("UPDATE projects SET next_number = next_number + 1 WHERE id = ?1")
        .bind(project.id)
        .execute(&mut *tx)
        .await?;
    let key = format!("{}-{}", project.key, n);

    let row: IssueRow = sqlx::query_as(&format!(
        "INSERT INTO issues
            (key, project_id, title, body, status, priority, type, assignee_id,
             forgejo_repo, forgejo_number, milestone, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'none', ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         RETURNING {ISSUE_COLS}"
    ))
    .bind(&key)
    .bind(project.id)
    .bind(title)
    .bind(body)
    .bind(status.as_str())
    .bind(if is_pr { "pr" } else { "issue" })
    .bind(assignee_id)
    .bind(repo)
    .bind(number)
    .bind(milestone)
    .bind(created_at)
    .bind(updated_at)
    .fetch_one(&mut *tx)
    .await?;

    for label in dedup(labels) {
        sqlx::query("INSERT OR IGNORE INTO issue_labels (issue_id, label) VALUES (?1, ?2)")
            .bind(row.id)
            .bind(&label)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
         VALUES (?1, 'created', NULL, ?2, ?3, NULL, ?4)",
    )
    .bind(row.id)
    .bind(format!("{repo}#{number}"))
    .bind(source.as_str())
    .bind(updated_at)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    hydrate(db, row).await
}

/// Insert or update a synced comment (matched by `(issue_id, external_id)`).
pub async fn upsert_synced_comment(
    db: &SqlitePool,
    issue_id: i64,
    external_id: i64,
    author_id: Option<i64>,
    body: &str,
    created_at: &str,
    updated_at: &str,
) -> AppResult<()> {
    // If a matching not-yet-linked local comment exists (our own writeback that
    // hasn't recorded its external_id yet), claim it instead of inserting a dup.
    let claimed = sqlx::query(
        "UPDATE comments SET external_id = ?2, updated_at = ?3
         WHERE id = (
             SELECT id FROM comments
             WHERE issue_id = ?1 AND external_id IS NULL AND body = ?4
             ORDER BY id DESC LIMIT 1
         )",
    )
    .bind(issue_id)
    .bind(external_id)
    .bind(updated_at)
    .bind(body)
    .execute(db)
    .await?
    .rows_affected();
    if claimed > 0 {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO comments (issue_id, author_id, body, external_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(issue_id, external_id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
    )
    .bind(issue_id)
    .bind(author_id)
    .bind(body)
    .bind(external_id)
    .bind(created_at)
    .bind(updated_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Create a breadboard-authored comment (no `external_id` until it is pushed).
pub async fn create_comment(
    db: &SqlitePool,
    issue_id: i64,
    author_id: Option<i64>,
    body: &str,
) -> AppResult<Comment> {
    let ts = now_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO comments (issue_id, author_id, body, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(issue_id)
    .bind(author_id)
    .bind(body)
    .bind(&ts)
    .fetch_one(db)
    .await?;
    Ok(Comment {
        id,
        issue_id,
        author_id,
        body: body.to_string(),
        external_id: None,
        created_at: parse_ts(&ts),
        updated_at: parse_ts(&ts),
    })
}

/// Attach a `forgejo_ref` to a breadboard-native issue after it has been
/// created on Forgejo, and log it.
pub async fn link_issue(
    db: &SqlitePool,
    issue_id: i64,
    repo: &str,
    number: i64,
) -> AppResult<Issue> {
    let ts = now_rfc3339();
    sqlx::query(
        "UPDATE issues SET forgejo_repo = ?2, forgejo_number = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(issue_id)
    .bind(repo)
    .bind(number)
    .bind(&ts)
    .execute(db)
    .await?;
    sqlx::query(
        "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
         VALUES (?1, 'created', NULL, ?2, 'breadboard', NULL, ?3)",
    )
    .bind(issue_id)
    .bind(format!("{repo}#{number}"))
    .bind(&ts)
    .execute(db)
    .await?;
    let row: IssueRow = sqlx::query_as(&format!("SELECT {ISSUE_COLS} FROM issues WHERE id = ?1"))
        .bind(issue_id)
        .fetch_one(db)
        .await?;
    hydrate(db, row).await
}

/// Claim a local, not-yet-linked issue that matches an incoming Forgejo issue
/// by project + exact title, created moments ago. This closes a race between
/// `writeback_new_issue`'s own `link_issue` call and the webhook for the issue
/// it just created arriving back at us first: without this, both paths insert
/// their own row for the same Forgejo issue, producing a duplicate.
pub async fn claim_unlinked_issue(
    db: &SqlitePool,
    project_id: i64,
    title: &str,
    repo: &str,
    number: i64,
) -> AppResult<Option<Issue>> {
    let cutoff = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let row: Option<IssueRow> = sqlx::query_as(&format!(
        "SELECT {ISSUE_COLS} FROM issues
         WHERE project_id = ?1 AND forgejo_repo IS NULL AND title = ?2 AND created_at >= ?3
         ORDER BY created_at DESC LIMIT 1"
    ))
    .bind(project_id)
    .bind(title)
    .bind(&cutoff)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    link_issue(db, row.id, repo, number).await.map(Some)
}

pub async fn set_comment_external_id(
    db: &SqlitePool,
    comment_id: i64,
    external_id: i64,
) -> AppResult<()> {
    sqlx::query("UPDATE comments SET external_id = ?2 WHERE id = ?1 AND external_id IS NULL")
        .bind(comment_id)
        .bind(external_id)
        .execute(db)
        .await?;
    Ok(())
}

// --- repo sync state ---------------------------------------------------

pub struct RepoSyncView {
    pub repo: String,
    pub project_key: Option<String>,
    pub enabled: bool,
    pub last_synced_at: Option<String>,
}

pub async fn list_repos(db: &SqlitePool) -> AppResult<Vec<RepoSyncView>> {
    let rows: Vec<(String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT r.forgejo_repo, r.enabled, r.last_synced_at, p.key
         FROM repo_sync_state r
         LEFT JOIN projects p ON p.forgejo_repo = r.forgejo_repo
         ORDER BY r.forgejo_repo",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(repo, enabled, last, key)| RepoSyncView {
            repo,
            project_key: key,
            enabled: enabled != 0,
            last_synced_at: last,
        })
        .collect())
}

pub async fn upsert_repo(db: &SqlitePool, repo: &str, enabled: bool) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO repo_sync_state (forgejo_repo, enabled) VALUES (?1, ?2)
         ON CONFLICT(forgejo_repo) DO UPDATE SET enabled = excluded.enabled",
    )
    .bind(repo)
    .bind(enabled as i64)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn enabled_repos(db: &SqlitePool) -> AppResult<Vec<(String, Option<String>)>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT forgejo_repo, last_synced_at FROM repo_sync_state WHERE enabled = 1",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

pub async fn repo_enabled(db: &SqlitePool, repo: &str) -> AppResult<bool> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT enabled FROM repo_sync_state WHERE forgejo_repo = ?1")
            .bind(repo)
            .fetch_optional(db)
            .await?;
    Ok(v == Some(1))
}

pub async fn set_repo_synced_at(db: &SqlitePool, repo: &str, ts: &str) -> AppResult<()> {
    sqlx::query("UPDATE repo_sync_state SET last_synced_at = ?2 WHERE forgejo_repo = ?1")
        .bind(repo)
        .bind(ts)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_issue(db: &SqlitePool, key: &str) -> AppResult<()> {
    let affected = sqlx::query("DELETE FROM issues WHERE key = ?1")
        .bind(key)
        .execute(db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn list_comments(db: &SqlitePool, issue_id: i64) -> AppResult<Vec<Comment>> {
    #[derive(FromRow)]
    struct Row {
        id: i64,
        issue_id: i64,
        author_id: Option<i64>,
        body: String,
        external_id: Option<i64>,
        created_at: String,
        updated_at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, issue_id, author_id, body, external_id, created_at, updated_at
         FROM comments WHERE issue_id = ?1 ORDER BY created_at",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Comment {
            id: r.id,
            issue_id: r.issue_id,
            author_id: r.author_id,
            body: r.body,
            external_id: r.external_id,
            created_at: parse_ts(&r.created_at),
            updated_at: parse_ts(&r.updated_at),
        })
        .collect())
}

/// One `sync_log` entry with the actor's login resolved, oldest first.
#[derive(serde::Serialize)]
pub struct ActivityEntry {
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub source: String,
    pub actor: Option<String>,
    pub at: chrono::DateTime<chrono::Utc>,
}

pub async fn issue_activity(db: &SqlitePool, issue_id: i64) -> AppResult<Vec<ActivityEntry>> {
    #[derive(FromRow)]
    struct Row {
        field: String,
        old_value: Option<String>,
        new_value: Option<String>,
        source: String,
        actor: Option<String>,
        at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT s.field, s.old_value, s.new_value, s.source, u.login AS actor, s.at
         FROM sync_log s LEFT JOIN users u ON u.id = s.actor_id
         WHERE s.issue_id = ?1 ORDER BY s.at, s.id",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ActivityEntry {
            field: r.field,
            old_value: r.old_value,
            new_value: r.new_value,
            source: r.source,
            actor: r.actor,
            at: parse_ts(&r.at),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// notifications
// ---------------------------------------------------------------------------

use breadboard_core::{Notification, NotificationKind};

pub async fn create_notification(
    db: &SqlitePool,
    user_id: i64,
    kind: NotificationKind,
    issue_id: Option<i64>,
    body: &str,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO notifications (user_id, kind, issue_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(user_id)
    .bind(kind.as_str())
    .bind(issue_id)
    .bind(body)
    .bind(now_rfc3339())
    .fetch_one(db)
    .await?;
    Ok(id)
}

pub async fn list_notifications(
    db: &SqlitePool,
    user_id: i64,
    unread_only: bool,
) -> AppResult<Vec<Notification>> {
    #[derive(FromRow)]
    struct Row {
        id: i64,
        user_id: i64,
        kind: String,
        issue_id: Option<i64>,
        body: String,
        read: i64,
        created_at: String,
    }
    let sql = format!(
        "SELECT id, user_id, kind, issue_id, body, read, created_at FROM notifications
         WHERE user_id = ?1 {} ORDER BY created_at DESC LIMIT 100",
        if unread_only { "AND read = 0" } else { "" }
    );
    let rows: Vec<Row> = sqlx::query_as(&sql).bind(user_id).fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| Notification {
            id: r.id,
            user_id: r.user_id,
            kind: NotificationKind::parse(&r.kind).unwrap_or(NotificationKind::Mention),
            issue_id: r.issue_id,
            body: r.body,
            read: r.read != 0,
            created_at: parse_ts(&r.created_at),
        })
        .collect())
}

pub async fn unread_count(db: &SqlitePool, user_id: i64) -> AppResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE user_id = ?1 AND read = 0")
            .bind(user_id)
            .fetch_one(db)
            .await?,
    )
}

pub async fn mark_notification_read(db: &SqlitePool, user_id: i64, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE notifications SET read = 1 WHERE id = ?1 AND user_id = ?2")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn mark_all_read(db: &SqlitePool, user_id: i64) -> AppResult<()> {
    sqlx::query("UPDATE notifications SET read = 1 WHERE user_id = ?1")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct NotificationPrefs {
    pub assigned: bool,
    pub mention: bool,
    pub cycle: bool,
    pub digest: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self { assigned: true, mention: true, cycle: true, digest: false }
    }
}

pub async fn get_prefs(db: &SqlitePool, user_id: i64) -> AppResult<NotificationPrefs> {
    let row: Option<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT assigned, mention, cycle, digest FROM notification_prefs WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some((a, m, c, d)) => NotificationPrefs {
            assigned: a != 0,
            mention: m != 0,
            cycle: c != 0,
            digest: d != 0,
        },
        None => NotificationPrefs::default(),
    })
}

pub async fn set_prefs(
    db: &SqlitePool,
    user_id: i64,
    p: NotificationPrefs,
) -> AppResult<NotificationPrefs> {
    sqlx::query(
        "INSERT INTO notification_prefs (user_id, assigned, mention, cycle, digest)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
            assigned = excluded.assigned, mention = excluded.mention,
            cycle = excluded.cycle, digest = excluded.digest",
    )
    .bind(user_id)
    .bind(p.assigned as i64)
    .bind(p.mention as i64)
    .bind(p.cycle as i64)
    .bind(p.digest as i64)
    .execute(db)
    .await?;
    Ok(p)
}

/// A user who opted into the daily digest, with the timestamp of their last
/// one (`None` if they have never had one).
pub struct DigestRecipient {
    pub user_id: i64,
    pub last_digest_at: Option<String>,
}

pub async fn digest_recipients(db: &SqlitePool) -> AppResult<Vec<DigestRecipient>> {
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT user_id, last_digest_at FROM notification_prefs WHERE digest = 1")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, last_digest_at)| DigestRecipient {
            user_id,
            last_digest_at,
        })
        .collect())
}

/// Unread notifications for a user created strictly after `since` (rfc3339),
/// oldest first. Digest rows themselves are excluded so a digest never
/// summarises earlier digests. `since = None` covers every unread item.
pub async fn unread_notifications_since(
    db: &SqlitePool,
    user_id: i64,
    since: Option<&str>,
) -> AppResult<Vec<Notification>> {
    #[derive(FromRow)]
    struct Row {
        id: i64,
        user_id: i64,
        kind: String,
        issue_id: Option<i64>,
        body: String,
        read: i64,
        created_at: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, user_id, kind, issue_id, body, read, created_at FROM notifications
         WHERE user_id = ?1 AND read = 0 AND kind != 'digest'
           AND (?2 IS NULL OR created_at > ?2)
         ORDER BY created_at ASC",
    )
    .bind(user_id)
    .bind(since)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Notification {
            id: r.id,
            user_id: r.user_id,
            kind: NotificationKind::parse(&r.kind).unwrap_or(NotificationKind::Mention),
            issue_id: r.issue_id,
            body: r.body,
            read: r.read != 0,
            created_at: parse_ts(&r.created_at),
        })
        .collect())
}

/// Record that a digest was just sent to `user_id`. Creates the prefs row if
/// somehow absent (it should already exist, since only `digest = 1` users get
/// here).
pub async fn set_last_digest_at(db: &SqlitePool, user_id: i64, at: &str) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO notification_prefs (user_id, last_digest_at) VALUES (?1, ?2)
         ON CONFLICT(user_id) DO UPDATE SET last_digest_at = excluded.last_digest_at",
    )
    .bind(user_id)
    .bind(at)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn find_user_by_login(db: &SqlitePool, login: &str) -> AppResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM users WHERE login = ?1 COLLATE NOCASE")
            .bind(login)
            .fetch_optional(db)
            .await?,
    )
}

pub async fn issue_assignees_in_cycle(db: &SqlitePool, cycle_id: i64) -> AppResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT assignee_id FROM issues WHERE cycle_id = ?1 AND assignee_id IS NOT NULL",
    )
    .bind(cycle_id)
    .fetch_all(db)
    .await?)
}

fn dedup(labels: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    labels
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && seen.insert(l.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// agent claims (exclusive server-side presence)
// ---------------------------------------------------------------------------

use breadboard_core::{AgentClaim, AgentState};

#[derive(FromRow)]
struct AgentClaimRow {
    issue_id: i64,
    key: String,
    issue_title: String,
    issue_status: String,
    claimed_by: i64,
    claimed_by_login: String,
    agent_name: String,
    session_id: Option<String>,
    state: String,
    note: Option<String>,
    branch: Option<String>,
    worktree: Option<String>,
    repo: Option<String>,
    pr_url: Option<String>,
    pr_number: Option<i64>,
    head_sha: Option<String>,
    board_status: String,
    claimed_at: String,
    updated_at: String,
    last_heartbeat: String,
}

impl From<AgentClaimRow> for AgentClaim {
    fn from(r: AgentClaimRow) -> Self {
        AgentClaim {
            issue_id: r.issue_id,
            key: r.key,
            issue_title: r.issue_title,
            issue_status: Status::parse(&r.issue_status).unwrap_or(Status::Doing),
            claimed_by: r.claimed_by,
            claimed_by_login: r.claimed_by_login,
            agent_name: r.agent_name,
            session_id: r.session_id,
            state: AgentState::parse(&r.state).unwrap_or(AgentState::Implementing),
            note: r.note,
            branch: r.branch,
            worktree: r.worktree,
            repo: r.repo,
            pr_url: r.pr_url,
            pr_number: r.pr_number,
            head_sha: r.head_sha,
            board_status: Status::parse(&r.board_status).unwrap_or(Status::Doing),
            claimed_at: parse_ts(&r.claimed_at),
            updated_at: parse_ts(&r.updated_at),
            last_heartbeat: parse_ts(&r.last_heartbeat),
        }
    }
}

const AGENT_CLAIM_COLS: &str = "c.issue_id, i.key AS key, i.title AS issue_title, \
    i.status AS issue_status, c.claimed_by, u.login AS claimed_by_login, \
    c.agent_name, c.session_id, c.state, c.note, c.branch, c.worktree, c.repo, \
    c.pr_url, c.pr_number, c.head_sha, c.board_status, c.claimed_at, c.updated_at, \
    c.last_heartbeat";

fn agent_claim_join() -> String {
    format!(
        "SELECT {AGENT_CLAIM_COLS} FROM agent_claims c \
         JOIN issues i ON i.id = c.issue_id \
         JOIN users u ON u.id = c.claimed_by"
    )
}

pub async fn get_claim(db: &SqlitePool, issue_id: i64) -> AppResult<Option<AgentClaim>> {
    let row: Option<AgentClaimRow> =
        sqlx::query_as(&format!("{} WHERE c.issue_id = ?1", agent_claim_join()))
            .bind(issue_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(Into::into))
}

pub async fn list_claims(db: &SqlitePool) -> AppResult<Vec<AgentClaim>> {
    let rows: Vec<AgentClaimRow> =
        sqlx::query_as(&format!("{} ORDER BY c.last_heartbeat DESC", agent_claim_join()))
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_claim(
    db: &SqlitePool,
    issue: &Issue,
    claimed_by: i64,
    input: &crate::dto::ClaimUpsert,
) -> AppResult<AgentClaim> {
    input.validate().map_err(AppError::BadRequest)?;
    let ts = now_rfc3339();
    let state = input.state.unwrap_or(AgentState::Implementing);
    let res = sqlx::query(
        "INSERT INTO agent_claims (issue_id, claimed_by, agent_name, session_id, state,
             note, branch, worktree, repo, pr_url, pr_number, head_sha, board_status,
             claimed_at, updated_at, last_heartbeat)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?14)",
    )
    .bind(issue.id)
    .bind(claimed_by)
    .bind(input.agent_name.trim())
    .bind(input.session_id.clone())
    .bind(state.as_str())
    .bind(input.note.clone())
    .bind(input.branch.clone())
    .bind(input.worktree.clone())
    .bind(input.repo.clone())
    .bind(input.pr_url.clone())
    .bind(input.pr_number)
    .bind(input.head_sha.clone())
    .bind(issue.status.as_str())
    .bind(&ts)
    .execute(db)
    .await;
    match res {
        Ok(_) => get_claim(db, issue.id).await?.ok_or(AppError::NotFound),
        Err(sqlx::Error::Database(dbe)) if dbe.is_unique_violation() => {
            Err(AppError::Conflict("issue is already claimed".into()))
        }
        Err(other) => Err(AppError::Db(other)),
    }
}

pub async fn update_claim(
    db: &SqlitePool,
    issue_id: i64,
    input: &crate::dto::ClaimUpdate,
) -> AppResult<AgentClaim> {
    let existing = get_claim(db, issue_id).await?.ok_or(AppError::NotFound)?;
    if let Some(agent_name) = &input.agent_name {
        if agent_name.trim().is_empty() {
            return Err(AppError::bad_request("agent_name must not be empty"));
        }
        sqlx::query("UPDATE agent_claims SET agent_name = ?2 WHERE issue_id = ?1")
            .bind(issue_id)
            .bind(agent_name.trim())
            .execute(db)
            .await?;
    }
    if let Some(state) = input.state {
        sqlx::query("UPDATE agent_claims SET state = ?2 WHERE issue_id = ?1")
            .bind(issue_id)
            .bind(state.as_str())
            .execute(db)
            .await?;
    }
    // Nullable fields: Some(None) clears, Some(Some(v)) sets, None leaves alone.
    macro_rules! set_nullable {
        ($col:literal, $val:expr) => {
            if let Some(v) = $val {
                sqlx::query(concat!("UPDATE agent_claims SET ", $col, " = ?2 WHERE issue_id = ?1"))
                    .bind(issue_id)
                    .bind(v.clone())
                    .execute(db)
                    .await?;
            }
        };
    }
    set_nullable!("session_id", &input.session_id);
    set_nullable!("note", &input.note);
    set_nullable!("branch", &input.branch);
    set_nullable!("worktree", &input.worktree);
    set_nullable!("repo", &input.repo);
    set_nullable!("pr_url", &input.pr_url);
    set_nullable!("pr_number", &input.pr_number);
    set_nullable!("head_sha", &input.head_sha);
    // Refresh board_status from the live issue + heartbeat.
    let issue = get_issue(db, existing.issue_id).await?;
    let ts = now_rfc3339();
    sqlx::query(
        "UPDATE agent_claims SET board_status = ?2, updated_at = ?3, last_heartbeat = ?3 WHERE issue_id = ?1",
    )
    .bind(issue_id)
    .bind(issue.status.as_str())
    .bind(&ts)
    .execute(db)
    .await?;
    get_claim(db, issue_id).await?.ok_or(AppError::NotFound)
}

pub async fn delete_claim(db: &SqlitePool, issue_id: i64) -> AppResult<bool> {
    let n = sqlx::query("DELETE FROM agent_claims WHERE issue_id = ?1")
        .bind(issue_id)
        .execute(db)
        .await?
        .rows_affected();
    Ok(n > 0)
}

pub async fn refresh_claim_board_status(
    db: &SqlitePool,
    issue_id: i64,
    status: Status,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE agent_claims SET board_status = ?2, updated_at = ?3 WHERE issue_id = ?1",
    )
    .bind(issue_id)
    .bind(status.as_str())
    .bind(now_rfc3339())
    .execute(db)
    .await?;
    Ok(())
}

/// Startup sanity check: there is deliberately no catch-all default project.
///
/// Seeding `ROAD` made it the silent destination for every title-only issue,
/// including work that belonged to a repo or to a project of its own. The
/// first issue on a fresh database now requires creating the right project
/// (`POST /api/projects`), which keeps repo trackers and native projects
/// honest.
pub async fn ensure_seed(db: &SqlitePool) -> anyhow::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
        .fetch_one(db)
        .await
        .context("counting projects")?;
    if count == 0 {
        tracing::info!(
            "no projects yet: create one with POST /api/projects before filing issues"
        );
    }
    Ok(())
}
