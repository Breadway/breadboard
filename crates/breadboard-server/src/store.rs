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
    let row: ProjectRow = sqlx::query_as(&format!(
        "SELECT {PROJECT_COLS} FROM projects WHERE id = ?1"
    ))
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

const EPIC_COLS: &str = "id, key, title, description, project_id, archived, created_at, updated_at";

pub async fn list_epics(db: &SqlitePool) -> AppResult<Vec<Epic>> {
    let rows: Vec<EpicRow> = sqlx::query_as(&format!("SELECT {EPIC_COLS} FROM epics ORDER BY key"))
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
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM issues WHERE epic_id = ?1 GROUP BY status")
            .bind(epic_id)
            .fetch_all(db)
            .await?;
    let count = |s: &str| {
        rows.iter()
            .find(|(st, _)| st == s)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    };
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

async fn hydrate_cycle(db: &SqlitePool, r: CycleRow) -> AppResult<Cycle> {
    let project_ids = sqlx::query_scalar(
        "SELECT project_id FROM cycle_projects WHERE cycle_id = ?1 ORDER BY project_id",
    )
    .bind(r.id)
    .fetch_all(db)
    .await?;
    Ok(Cycle {
        id: r.id,
        name: r.name,
        starts_on: r.starts_on.parse().unwrap_or_default(),
        ends_on: r.ends_on.parse().unwrap_or_default(),
        closed: r.closed != 0,
        project_ids,
    })
}

const CYCLE_COLS: &str = "id, name, starts_on, ends_on, closed";

pub async fn list_cycles(db: &SqlitePool) -> AppResult<Vec<Cycle>> {
    let rows: Vec<CycleRow> = sqlx::query_as(&format!(
        "SELECT {CYCLE_COLS} FROM cycles ORDER BY starts_on DESC"
    ))
    .fetch_all(db)
    .await?;
    let mut cycles = Vec::with_capacity(rows.len());
    for row in rows {
        cycles.push(hydrate_cycle(db, row).await?);
    }
    Ok(cycles)
}

pub async fn get_cycle(db: &SqlitePool, id: i64) -> AppResult<Cycle> {
    let row: CycleRow = sqlx::query_as(&format!("SELECT {CYCLE_COLS} FROM cycles WHERE id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(AppError::NotFound)?;
    hydrate_cycle(db, row).await
}

pub async fn create_cycle(db: &SqlitePool, new: &NewCycle) -> AppResult<Cycle> {
    if new.name.trim().is_empty() {
        return Err(AppError::bad_request("name is required"));
    }
    if new.ends_on < new.starts_on {
        return Err(AppError::bad_request(
            "ends_on must be on or after starts_on",
        ));
    }
    let mut tx = crate::db::begin_write(db).await?;
    let row: CycleRow = sqlx::query_as(&format!(
        "INSERT INTO cycles (name, starts_on, ends_on) VALUES (?1, ?2, ?3) RETURNING {CYCLE_COLS}"
    ))
    .bind(&new.name)
    .bind(new.starts_on.to_string())
    .bind(new.ends_on.to_string())
    .fetch_one(&mut *tx)
    .await?;
    for project_id in new
        .project_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id = ?1")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
        if exists == 0 {
            return Err(AppError::bad_request("unknown project id"));
        }
        sqlx::query("INSERT INTO cycle_projects (cycle_id, project_id) VALUES (?1, ?2)")
            .bind(row.id)
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    get_cycle(db, row.id).await
}

pub async fn update_cycle_projects(
    db: &SqlitePool,
    id: i64,
    project_ids: &[i64],
) -> AppResult<Cycle> {
    let mut tx = crate::db::begin_write(db).await?;
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cycles WHERE id = ?1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound);
    }
    for project_id in project_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE id = ?1")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
        if exists == 0 {
            return Err(AppError::bad_request("unknown project id"));
        }
    }
    sqlx::query("DELETE FROM cycle_projects WHERE cycle_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for project_id in project_ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        sqlx::query("INSERT INTO cycle_projects (cycle_id, project_id) VALUES (?1, ?2)")
            .bind(id)
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    get_cycle(db, id).await
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
pub const SYNCED_FIELDS: &[&str] = &[
    "title",
    "body",
    "status",
    "assignee_id",
    "labels",
    "milestone",
];

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
    let row: IssueRow = sqlx::query_as(&format!("SELECT {ISSUE_COLS} FROM issues WHERE key = ?1"))
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

    let mut tx = crate::db::begin_write(db).await?;

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

    let cycle_id = if let Some(explicit) = new.cycle_id {
        Some(explicit)
    } else {
        let matches: Vec<i64> = sqlx::query_scalar(
            "SELECT c.id FROM cycles c JOIN cycle_projects cp ON cp.cycle_id = c.id \
             WHERE cp.project_id = ?1 AND c.closed = 0 ORDER BY c.id",
        )
        .bind(project.id)
        .fetch_all(&mut *tx)
        .await?;
        match matches.as_slice() {
            [] => None,
            [id] => Some(*id),
            _ => {
                return Err(AppError::Conflict(format!(
                    "project {} is selected by multiple open cycles; specify cycle_id explicitly",
                    project.key
                )))
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
    .bind(cycle_id)
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
    let mut tx = crate::db::begin_write(db).await?;
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
            set_col!(
                "title",
                title.trim(),
                Some(before.title.clone()),
                Some(title.clone())
            );
        }
    }
    if let Some(body) = &patch.body {
        if *body != before.body {
            set_col!(
                "body",
                body.as_str(),
                Some(before.body.clone()),
                Some(body.clone())
            );
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

    let mut tx = crate::db::begin_write(db).await?;
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
    sqlx::query(
        "INSERT OR IGNORE INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, ?3)",
    )
    .bind(issue_id)
    .bind(target_id)
    .bind(kind.as_str())
    .execute(db)
    .await?;
    Ok(())
}

/// Record a PR's `closes`/`closed-by` links to one target and, when `status`
/// differs, apply the cascaded status move in the same transaction.
///
/// Links and closing status are written atomically: a busy database or a
/// mid-write error can never leave a card linked but not closed (or closed
/// without the link that explains why). Returns the updated target only when a
/// status change was applied.
pub async fn sync_pr_close_target(
    db: &SqlitePool,
    pr_issue_id: i64,
    target_id: i64,
    status: Option<Status>,
    source: SyncSource,
) -> AppResult<Option<Issue>> {
    // Reconcile calls this repeatedly for unchanged PRs. Skip taking the write
    // lock when both links and the status already match.
    let links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM issue_links \
         WHERE (issue_id = ?1 AND target_id = ?2 AND kind = 'closes') \
            OR (issue_id = ?2 AND target_id = ?1 AND kind = 'closed-by')",
    )
    .bind(pr_issue_id)
    .bind(target_id)
    .fetch_one(db)
    .await?;
    let current: Option<String> = sqlx::query_scalar("SELECT status FROM issues WHERE id = ?1")
        .bind(target_id)
        .fetch_optional(db)
        .await?;
    let Some(current) = current else {
        return Ok(None);
    };
    let needs_status = status.is_some_and(|s| s.as_str() != current);
    if links >= 2 && !needs_status {
        return Ok(None);
    }

    let mut tx = crate::db::begin_write(db).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, 'closes')",
    )
    .bind(pr_issue_id)
    .bind(target_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, 'closed-by')",
    )
    .bind(target_id)
    .bind(pr_issue_id)
    .execute(&mut *tx)
    .await?;

    let Some(status) = status else {
        tx.commit().await?;
        return Ok(None);
    };
    // Re-read inside the write transaction: a concurrent writer may have moved
    // the target between the optimistic check above and acquiring the lock.
    let (key, current): (String, String) =
        sqlx::query_as("SELECT key, status FROM issues WHERE id = ?1")
            .bind(target_id)
            .fetch_one(&mut *tx)
            .await?;
    if current == status.as_str() {
        tx.commit().await?;
        return Ok(None);
    }

    let ts = now_rfc3339();
    sqlx::query("UPDATE issues SET status = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(target_id)
        .bind(status.as_str())
        .bind(&ts)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
         VALUES (?1, 'status', ?2, ?3, ?4, NULL, ?5)",
    )
    .bind(target_id)
    .bind(&current)
    .bind(status.as_str())
    .bind(source.as_str())
    .bind(&ts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let _ = sqlx::query(
        "UPDATE agent_claims SET board_status = ?2, updated_at = ?3 WHERE issue_id = ?1",
    )
    .bind(target_id)
    .bind(status.as_str())
    .bind(&ts)
    .execute(db)
    .await;

    let after = get_issue_by_key(db, &key).await?;
    Ok(Some(after))
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
    let mut tx = crate::db::begin_write(db).await?;
    // Only genuinely new, current issues inherit defaults. Reconcile imports
    // of older backlog are historical and must not be retroactively scheduled;
    // PRs are read-mostly and never inherit issue scheduling defaults.
    let cycle_id = if !is_pr && is_current_external_issue(created_at, updated_at) {
        let matches: Vec<i64> = sqlx::query_scalar(
            "SELECT c.id FROM cycles c JOIN cycle_projects cp ON cp.cycle_id = c.id \
             WHERE cp.project_id = ?1 AND c.closed = 0 ORDER BY c.id",
        )
        .bind(project.id)
        .fetch_all(&mut *tx)
        .await?;
        match matches.as_slice() {
            [] => None,
            [id] => Some(*id),
            _ => None, // Ambiguous defaults require an explicit local choice.
        }
    } else {
        None
    };
    let n: i64 = project.next_number;
    sqlx::query("UPDATE projects SET next_number = next_number + 1 WHERE id = ?1")
        .bind(project.id)
        .execute(&mut *tx)
        .await?;
    let key = format!("{}-{}", project.key, n);

    let row: IssueRow = sqlx::query_as(&format!(
        "INSERT INTO issues
            (key, project_id, title, body, status, priority, type, assignee_id, cycle_id,
             forgejo_repo, forgejo_number, milestone, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'none', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         RETURNING {ISSUE_COLS}"
    ))
    .bind(&key)
    .bind(project.id)
    .bind(title)
    .bind(body)
    .bind(status.as_str())
    .bind(if is_pr { "pr" } else { "issue" })
    .bind(assignee_id)
    .bind(cycle_id)
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

fn is_current_external_issue(created_at: &str, updated_at: &str) -> bool {
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return false;
    };
    let Ok(updated) = chrono::DateTime::parse_from_rfc3339(updated_at) else {
        return false;
    };
    let now = chrono::Utc::now();
    created.with_timezone(&chrono::Utc) >= now - chrono::Duration::days(30)
        && updated.with_timezone(&chrono::Utc) >= now - chrono::Duration::days(30)
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
    let mut tx = crate::db::begin_write(db).await?;
    let result = sqlx::query(
        "UPDATE issues SET forgejo_repo = ?2, forgejo_number = ?3, updated_at = ?4 WHERE id = ?1 AND forgejo_repo IS NULL",
    ).bind(issue_id).bind(repo).bind(number).bind(&ts).execute(&mut *tx).await;
    // A concurrent webhook may have linked the exact same remote issue. Only
    // interpret a uniqueness conflict as success when that link exists.
    if let Err(error) = result {
        tx.rollback().await?;
        if let Some(already) = find_issue_by_forgejo(db, repo, number).await? {
            return Ok(already);
        }
        return Err(error.into());
    }
    let affected = result.unwrap().rows_affected();
    if affected == 0 {
        tx.rollback().await?;
        if let Some(already) = find_issue_by_forgejo(db, repo, number).await? {
            return Ok(already);
        }
        return Err(AppError::Conflict("issue was linked concurrently".into()));
    }
    sqlx::query("INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at) VALUES (?1, 'created', NULL, ?2, 'breadboard', NULL, ?3)")
        .bind(issue_id).bind(format!("{repo}#{number}")).bind(&ts).execute(&mut *tx).await?;
    tx.commit().await?;
    let row: IssueRow = sqlx::query_as(&format!("SELECT {ISSUE_COLS} FROM issues WHERE id = ?1"))
        .bind(issue_id)
        .fetch_one(db)
        .await?;
    hydrate(db, row).await
}

/// Claim a local, not-yet-linked issue that matches an incoming Forgejo issue
/// by project + exact title, regardless of age. This closes a race between
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
    let mut rows: Vec<IssueRow> = sqlx::query_as(&format!(
        "SELECT {ISSUE_COLS} FROM issues
         WHERE project_id = ?1 AND forgejo_repo IS NULL AND title = ?2
         ORDER BY created_at DESC"
    ))
    .bind(project_id)
    .bind(title)
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    rows.truncate(1);
    match link_issue(db, rows[0].id, repo, number).await {
        Ok(issue) => Ok(Some(issue)),
        Err(_) => find_issue_by_forgejo(db, repo, number).await,
    }
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
        Self {
            assigned: true,
            mention: true,
            cycle: true,
            digest: false,
        }
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
// agent claims (exclusive work + soft intents)
// ---------------------------------------------------------------------------

use breadboard_core::{AgentClaim, AgentState, ClaimMode};

#[derive(FromRow)]
struct AgentClaimRow {
    id: i64,
    issue_id: i64,
    key: String,
    issue_title: String,
    issue_status: String,
    claimed_by: i64,
    claimed_by_login: String,
    agent_name: String,
    agent_message: Option<String>,
    mode: String,
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
            id: r.id,
            issue_id: r.issue_id,
            key: r.key,
            issue_title: r.issue_title,
            issue_status: Status::parse(&r.issue_status).unwrap_or(Status::Doing),
            claimed_by: r.claimed_by,
            claimed_by_login: r.claimed_by_login,
            agent_name: r.agent_name,
            message: r.agent_message,
            mode: ClaimMode::parse(&r.mode).unwrap_or(ClaimMode::Exclusive),
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

const AGENT_CLAIM_COLS: &str = "c.id, c.issue_id, i.key AS key, i.title AS issue_title, \
    i.status AS issue_status, c.claimed_by, u.login AS claimed_by_login, \
    c.agent_name, p.message AS agent_message, c.mode, c.session_id, c.state, c.note, c.branch, c.worktree, c.repo, \
    c.pr_url, c.pr_number, c.head_sha, c.board_status, c.claimed_at, c.updated_at, \
    c.last_heartbeat";

fn agent_claim_join() -> String {
    format!(
        "SELECT {AGENT_CLAIM_COLS} FROM agent_claims c \
         JOIN issues i ON i.id = c.issue_id \
         JOIN users u ON u.id = c.claimed_by \
         LEFT JOIN agent_profiles p ON p.user_id = c.claimed_by AND p.agent_name = c.agent_name"
    )
}

/// True when the issue currently has an exclusive claim.
pub async fn has_exclusive_claim(db: &SqlitePool, issue_id: i64) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_claims WHERE issue_id = ?1 AND mode = 'exclusive'",
    )
    .bind(issue_id)
    .fetch_one(db)
    .await?;
    Ok(n > 0)
}

pub async fn get_claim_by_id(db: &SqlitePool, id: i64) -> AppResult<Option<AgentClaim>> {
    let row: Option<AgentClaimRow> =
        sqlx::query_as(&format!("{} WHERE c.id = ?1", agent_claim_join()))
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(Into::into))
}

/// Every claim (exclusive first) for one issue.
pub async fn list_issue_claims(db: &SqlitePool, issue_id: i64) -> AppResult<Vec<AgentClaim>> {
    let rows: Vec<AgentClaimRow> = sqlx::query_as(&format!(
        "{} WHERE c.issue_id = ?1 ORDER BY c.mode = 'intent', c.id",
        agent_claim_join()
    ))
    .bind(issue_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn list_claims(db: &SqlitePool) -> AppResult<Vec<AgentClaim>> {
    let rows: Vec<AgentClaimRow> = sqlx::query_as(&format!(
        "{} ORDER BY c.mode = 'intent', c.last_heartbeat DESC",
        agent_claim_join()
    ))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Claims owned by `user_id` on an issue, optionally narrowed to one agent.
pub async fn find_user_claims(
    db: &SqlitePool,
    issue_id: i64,
    user_id: i64,
    agent: Option<&str>,
) -> AppResult<Vec<AgentClaim>> {
    let mut claims = list_issue_claims(db, issue_id).await?;
    claims.retain(|c| c.claimed_by == user_id);
    if let Some(agent) = agent {
        claims.retain(|c| c.agent_name.eq_ignore_ascii_case(agent));
    }
    Ok(claims)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_claim(
    db: &SqlitePool,
    issue: &Issue,
    claimed_by: i64,
    input: &crate::dto::ClaimUpsert,
) -> AppResult<AgentClaim> {
    input.validate().map_err(AppError::BadRequest)?;
    let mode = input.mode.unwrap_or(ClaimMode::Exclusive);
    let ts = now_rfc3339();
    let state = input.state.unwrap_or(AgentState::Implementing);

    if mode == ClaimMode::Exclusive && has_exclusive_claim(db, issue.id).await? {
        return Err(AppError::Conflict("issue is already claimed".into()));
    }

    // Taking the work makes the same agent's queued intent redundant, and
    // leaving both would make "resolve my claim by key" ambiguous.
    if mode == ClaimMode::Exclusive {
        sqlx::query(
            "DELETE FROM agent_claims
              WHERE issue_id = ?1 AND claimed_by = ?2 AND mode = 'intent'
                AND lower(agent_name) = lower(?3)",
        )
        .bind(issue.id)
        .bind(claimed_by)
        .bind(input.agent_name.trim())
        .execute(db)
        .await?;
    }

    // An intent re-claimed by the same agent is an update, not a duplicate.
    if mode == ClaimMode::Intent {
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM agent_claims
              WHERE issue_id = ?1 AND claimed_by = ?2 AND agent_name = ?3 AND mode = 'intent'",
        )
        .bind(issue.id)
        .bind(claimed_by)
        .bind(input.agent_name.trim())
        .fetch_optional(db)
        .await?;
        if let Some(id) = existing {
            // A re-claim refreshes the fields it carries and leaves the rest.
            let upd = crate::dto::ClaimUpdate {
                agent_name: Some(input.agent_name.trim().to_string()),
                state: input.state,
                session_id: input.session_id.clone().map(Some),
                note: input.note.clone().map(Some),
                branch: input.branch.clone().map(Some),
                worktree: input.worktree.clone().map(Some),
                repo: input.repo.clone().map(Some),
                pr_url: input.pr_url.clone().map(Some),
                pr_number: input.pr_number.map(Some),
                head_sha: input.head_sha.clone().map(Some),
                message: input.message.clone().map(Some),
            };
            let claim = update_claim_row(db, id, &upd, &ts).await?;
            return Ok(claim);
        }
    }

    let res = sqlx::query(
        "INSERT INTO agent_claims (issue_id, claimed_by, agent_name, mode, session_id, state,
             note, branch, worktree, repo, pr_url, pr_number, head_sha, board_status,
             claimed_at, updated_at, last_heartbeat)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15, ?15)",
    )
    .bind(issue.id)
    .bind(claimed_by)
    .bind(input.agent_name.trim().to_lowercase())
    .bind(mode.as_str())
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
        Ok(done) => {
            if let Some(message) = &input.message {
                let normalized = breadboard_core::dto::normalize_agent_message(message)
                    .map_err(AppError::BadRequest)?;
                set_agent_message(
                    db,
                    claimed_by,
                    &input.agent_name.trim().to_lowercase(),
                    normalized.as_deref(),
                )
                .await?;
            }
            get_claim_by_id(db, done.last_insert_rowid())
                .await?
                .ok_or(AppError::NotFound)
        }
        Err(sqlx::Error::Database(dbe)) if dbe.is_unique_violation() => {
            // Two concurrent intent claims: the loser updates the winner's
            // row instead of surfacing a conflict for a soft reservation.
            if mode == ClaimMode::Intent {
                let existing: Option<i64> = sqlx::query_scalar(
                    "SELECT id FROM agent_claims
                      WHERE issue_id = ?1 AND claimed_by = ?2
                        AND lower(agent_name) = lower(?3) AND mode = 'intent'",
                )
                .bind(issue.id)
                .bind(claimed_by)
                .bind(input.agent_name.trim())
                .fetch_optional(db)
                .await?;
                if let Some(id) = existing {
                    let upd = crate::dto::ClaimUpdate {
                        agent_name: Some(input.agent_name.trim().to_string()),
                        state: input.state,
                        session_id: input.session_id.clone().map(Some),
                        note: input.note.clone().map(Some),
                        branch: input.branch.clone().map(Some),
                        worktree: input.worktree.clone().map(Some),
                        repo: input.repo.clone().map(Some),
                        pr_url: input.pr_url.clone().map(Some),
                        pr_number: input.pr_number.map(Some),
                        head_sha: input.head_sha.clone().map(Some),
                        message: input.message.clone().map(Some),
                    };
                    return update_claim_row(db, id, &upd, &ts).await;
                }
            }
            Err(AppError::Conflict("issue is already claimed".into()))
        }
        Err(other) => Err(AppError::Db(other)),
    }
}

/// Apply claim fields to one row. `ts` is the heartbeat timestamp.
async fn update_claim_row(
    db: &SqlitePool,
    id: i64,
    input: &crate::dto::ClaimUpdate,
    ts: &str,
) -> AppResult<AgentClaim> {
    // One transaction so a partial multi-column update can never leave a
    // half-applied claim behind, and the heartbeat is always refreshed with
    // the changes it accompanies.
    let mut tx = crate::db::begin_write(db).await?;

    if let Some(agent_name) = &input.agent_name {
        let name = agent_name.trim();
        if name.is_empty() {
            return Err(AppError::bad_request("agent_name must not be empty"));
        }
        if name.len() > 64 {
            return Err(AppError::bad_request("agent_name must be 64 chars or fewer"));
        }
        sqlx::query("UPDATE agent_claims SET agent_name = ?2 WHERE id = ?1")
            .bind(id)
            .bind(name.to_lowercase())
            .execute(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
                    AppError::Conflict("another claim already uses that agent name".into())
                }
                other => AppError::Db(other),
            })?;
    }
    if let Some(state) = input.state {
        sqlx::query("UPDATE agent_claims SET state = ?2 WHERE id = ?1")
            .bind(id)
            .bind(state.as_str())
            .execute(&mut *tx)
            .await?;
    }
    // Nullable fields: Some(None) clears, Some(Some(v)) sets, None leaves alone.
    macro_rules! set_nullable {
        ($col:literal, $val:expr) => {
            if let Some(v) = $val {
                sqlx::query(concat!("UPDATE agent_claims SET ", $col, " = ?2 WHERE id = ?1"))
                    .bind(id)
                    .bind(v.clone())
                    .execute(&mut *tx)
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

    // Refresh board_status from the live issue + heartbeat in the same tx.
    let (issue_id, claimed_by, agent_name): (i64, i64, String) = sqlx::query_as(
        "SELECT issue_id, claimed_by, agent_name FROM agent_claims WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let status: String = sqlx::query_scalar("SELECT status FROM issues WHERE id = ?1")
        .bind(issue_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE agent_claims SET board_status = ?2, updated_at = ?3, last_heartbeat = ?3 WHERE id = ?1",
    )
    .bind(id)
    .bind(&status)
    .bind(ts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    if let Some(message) = &input.message {
        let normalized = breadboard_core::dto::normalize_agent_message(
            message.as_deref().unwrap_or_default(),
        )
        .map_err(AppError::BadRequest)?;
        set_agent_message(db, claimed_by, &agent_name, normalized.as_deref()).await?;
    }

    get_claim_by_id(db, id).await?.ok_or(AppError::NotFound)
}

pub async fn update_claim(
    db: &SqlitePool,
    claim_id: i64,
    input: &crate::dto::ClaimUpdate,
) -> AppResult<AgentClaim> {
    update_claim_row(db, claim_id, input, &now_rfc3339()).await
}

/// Release one claim by id. Returns the claim that was removed.
pub async fn delete_claim_by_id(db: &SqlitePool, id: i64) -> AppResult<Option<AgentClaim>> {
    let claim = get_claim_by_id(db, id).await?;
    if claim.is_some() {
        sqlx::query("DELETE FROM agent_claims WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(claim)
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

/// Upsert or clear an agent's one-liner. `agent_name` is normalised lowercase.
pub async fn set_agent_message(
    db: &SqlitePool,
    user_id: i64,
    agent_name: &str,
    message: Option<&str>,
) -> AppResult<()> {
    let name = agent_name.trim().to_lowercase();
    match message.filter(|m| !m.trim().is_empty()) {
        Some(message) => {
            sqlx::query(
                "INSERT INTO agent_profiles (user_id, agent_name, message, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id, agent_name) DO UPDATE SET
                    message = excluded.message, updated_at = excluded.updated_at",
            )
            .bind(user_id)
            .bind(&name)
            .bind(message.trim())
            .bind(now_rfc3339())
            .execute(db)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM agent_profiles WHERE user_id = ?1 AND agent_name = ?2")
                .bind(user_id)
                .bind(&name)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

pub async fn get_agent_message(
    db: &SqlitePool,
    user_id: i64,
    agent_name: &str,
) -> AppResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT message FROM agent_profiles WHERE user_id = ?1 AND agent_name = ?2",
    )
    .bind(user_id)
    .bind(agent_name.trim().to_lowercase())
    .fetch_optional(db)
    .await?)
}

/// Append a claim lifecycle row to the issue's activity timeline. Heartbeats
/// that change nothing are not logged, so this stays readable.
pub async fn log_claim_event(
    db: &SqlitePool,
    issue_id: i64,
    field: &str,
    old_value: Option<&str>,
    new_value: Option<&str>,
    actor_id: Option<i64>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
         VALUES (?1, ?2, ?3, ?4, 'breadboard', ?5, ?6)",
    )
    .bind(issue_id)
    .bind(field)
    .bind(old_value)
    .bind(new_value)
    .bind(actor_id)
    .bind(now_rfc3339())
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// chat
// ---------------------------------------------------------------------------

use breadboard_core::{ChatAuthorKind, ChatMessage, ChatPushKind, ChatRoom, ChatRoomKind};

pub fn agent_member(name: &str) -> String {
    format!("agent:{}", name.trim().to_lowercase())
}

pub fn human_member(login: &str) -> String {
    format!("human:{}", login.trim().to_lowercase())
}

#[derive(FromRow)]
struct ChatRoomRow {
    id: i64,
    kind: String,
    name: Option<String>,
    description: Option<String>,
}

#[derive(FromRow)]
struct ChatMessageRow {
    id: i64,
    room_id: i64,
    author_member: String,
    author_display: String,
    author_kind: String,
    body: String,
    created_at: String,
    message_kind: String,
}

impl From<ChatMessageRow> for ChatMessage {
    fn from(r: ChatMessageRow) -> Self {
        ChatMessage {
            id: r.id,
            room_id: r.room_id,
            author_member: r.author_member,
            author_display: r.author_display,
            author_kind: ChatAuthorKind::parse(&r.author_kind).unwrap_or(ChatAuthorKind::Human),
            message_kind: breadboard_core::ChatMessageKind::parse(&r.message_kind)
                .unwrap_or_default(),
            body: r.body,
            attachments: Vec::new(),
            created_at: parse_ts(&r.created_at),
        }
    }
}

#[derive(FromRow)]
struct ChatUploadRow {
    id: i64,
    filename: String,
    mime: String,
    size: i64,
}

#[derive(FromRow)]
struct ChatUploadJoinRow {
    message_id: i64,
    id: i64,
    filename: String,
    mime: String,
    size: i64,
}

impl From<ChatUploadRow> for breadboard_core::ChatUpload {
    fn from(r: ChatUploadRow) -> Self {
        breadboard_core::ChatUpload {
            id: r.id,
            filename: r.filename,
            mime: r.mime,
            size: r.size,
        }
    }
}

/// Attachments for a set of messages, in one query.
async fn attachments_for(
    db: &SqlitePool,
    message_ids: &[i64],
) -> AppResult<std::collections::HashMap<i64, Vec<breadboard_core::ChatUpload>>> {
    let mut out: std::collections::HashMap<i64, Vec<breadboard_core::ChatUpload>> =
        std::collections::HashMap::new();
    if message_ids.is_empty() {
        return Ok(out);
    }
    let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT message_id, id, filename, mime, size FROM chat_uploads WHERE message_id IN (",
    );
    let mut sep = qb.separated(", ");
    for id in message_ids {
        sep.push_bind(id);
    }
    qb.push(") ORDER BY id");
    let rows: Vec<ChatUploadJoinRow> = qb.build_query_as().fetch_all(db).await?;
    for row in rows {
        let message_id = row.message_id;
        out.entry(message_id).or_default().push(
            ChatUploadRow {
                id: row.id,
                filename: row.filename,
                mime: row.mime,
                size: row.size,
            }
            .into(),
        );
    }
    Ok(out)
}

async fn hydrate_messages(db: &SqlitePool, rows: Vec<ChatMessageRow>) -> AppResult<Vec<ChatMessage>> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut attachments = attachments_for(db, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut message: ChatMessage = row.into();
            message.attachments = attachments.remove(&message.id).unwrap_or_default();
            message
        })
        .collect())
}

const CHAT_ROOM_COLS: &str = "id, kind, name, description";

async fn chat_members(db: &SqlitePool, room_id: i64) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT member FROM chat_room_members WHERE room_id = ?1 ORDER BY member",
    )
    .bind(room_id)
    .fetch_all(db)
    .await?)
}

async fn hydrate_room(
    db: &SqlitePool,
    row: ChatRoomRow,
    reader: Option<&str>,
) -> AppResult<ChatRoom> {
    let members = chat_members(db, row.id).await?;
    let unread = match reader {
        Some(member) => chat_unread(db, row.id, member).await?,
        None => 0,
    };
    let last: Option<ChatMessageRow> = sqlx::query_as(
        "SELECT id, room_id, author_member, author_display, author_kind, body, created_at, message_kind
           FROM chat_messages
          WHERE room_id = ?1 AND deleted_at IS NULL AND compacted_at IS NULL
          ORDER BY id DESC LIMIT 1",
    )
    .bind(row.id)
    .fetch_optional(db)
    .await?;
    let last_message = match last {
        Some(row) => hydrate_messages(db, vec![row]).await?.pop(),
        None => None,
    };
    Ok(ChatRoom {
        id: row.id,
        kind: ChatRoomKind::parse(&row.kind).unwrap_or(ChatRoomKind::Room),
        name: row.name,
        description: row.description,
        members,
        unread,
        last_message,
    })
}

/// All shared rooms, plus DMs the viewer belongs to. `viewer = None` (a
/// human) sees everything, including every DM.
pub async fn chat_rooms(
    db: &SqlitePool,
    viewer: Option<&str>,
    reader: Option<&str>,
) -> AppResult<Vec<ChatRoom>> {
    let rows: Vec<ChatRoomRow> = sqlx::query_as(&format!(
        "SELECT {CHAT_ROOM_COLS} FROM chat_rooms ORDER BY kind, name, id"
    ))
    .fetch_all(db)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        if let Some(viewer) = viewer {
            if row.kind == ChatRoomKind::Dm.as_str() {
                let is_member: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM chat_room_members WHERE room_id = ?1 AND member = ?2",
                )
                .bind(row.id)
                .bind(viewer)
                .fetch_one(db)
                .await?;
                if is_member == 0 {
                    continue;
                }
            }
        }
        out.push(hydrate_room(db, row, reader).await?);
    }
    Ok(out)
}

pub async fn get_chat_room(db: &SqlitePool, id: i64) -> AppResult<ChatRoom> {
    let row: ChatRoomRow = sqlx::query_as(&format!(
        "SELECT {CHAT_ROOM_COLS} FROM chat_rooms WHERE id = ?1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    hydrate_room(db, row, None).await
}

pub async fn chat_room_by_name(db: &SqlitePool, name: &str) -> AppResult<Option<ChatRoom>> {
    let row: Option<ChatRoomRow> = sqlx::query_as(&format!(
        "SELECT {CHAT_ROOM_COLS} FROM chat_rooms WHERE name = ?1 AND kind = 'room'"
    ))
    .bind(name)
    .fetch_optional(db)
    .await?;
    match row {
        Some(row) => Ok(Some(hydrate_room(db, row, None).await?)),
        None => Ok(None),
    }
}

pub async fn create_chat_room(
    db: &SqlitePool,
    name: &str,
    description: Option<&str>,
) -> AppResult<ChatRoom> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_rooms (kind, name, description, created_at)
         VALUES ('room', ?1, ?2, ?3) RETURNING id",
    )
    .bind(name)
    .bind(description)
    .bind(now_rfc3339())
    .fetch_one(db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
            AppError::Conflict(format!("room `{name}` already exists"))
        }
        other => AppError::Db(other),
    })?;
    get_chat_room(db, id).await
}

pub async fn chat_unread(db: &SqlitePool, room_id: i64, member: &str) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages
          WHERE room_id = ?1
            AND id > COALESCE(
                (SELECT last_read_message_id FROM chat_reads WHERE room_id = ?1 AND member = ?2), 0)
            AND author_member != ?2
            AND deleted_at IS NULL AND compacted_at IS NULL",
    )
    .bind(room_id)
    .bind(member)
    .fetch_one(db)
    .await?)
}

pub async fn chat_mark_read(
    db: &SqlitePool,
    room_id: i64,
    member: &str,
    message_id: i64,
) -> AppResult<()> {
    if message_id <= 0 {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO chat_reads (room_id, member, last_read_message_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(room_id, member) DO UPDATE SET
            last_read_message_id = MAX(chat_reads.last_read_message_id, excluded.last_read_message_id),
            updated_at = excluded.updated_at",
    )
    .bind(room_id)
    .bind(member)
    .bind(message_id)
    .bind(now_rfc3339())
    .execute(db)
    .await?;
    Ok(())
}

pub async fn set_chat_push_sub(
    db: &SqlitePool,
    user_id: i64,
    member: &str,
    room_id: i64,
    enabled: bool,
) -> AppResult<()> {
    if enabled {
        sqlx::query(
            "INSERT OR IGNORE INTO chat_push_subs (user_id, member, room_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(user_id)
        .bind(member)
        .bind(room_id)
        .bind(now_rfc3339())
        .execute(db)
        .await?;
    } else {
        sqlx::query("DELETE FROM chat_push_subs WHERE user_id = ?1 AND member = ?2 AND room_id = ?3")
            .bind(user_id)
            .bind(member)
            .bind(room_id)
            .execute(db)
            .await?;
    }
    Ok(())
}

pub async fn chat_push_subs(db: &SqlitePool, user_id: i64, member: &str) -> AppResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT room_id FROM chat_push_subs WHERE user_id = ?1 AND member = ?2 ORDER BY room_id",
    )
    .bind(user_id)
    .bind(member)
    .fetch_all(db)
    .await?)
}

/// Owners of an agent name (it may exist via a profile, claims, or both).
async fn agent_owners(db: &SqlitePool, agent_name: &str) -> AppResult<Vec<i64>> {
    let name = agent_name.trim().to_lowercase();
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT user_id FROM (
            SELECT user_id FROM agent_profiles WHERE agent_name = ?1
            UNION
            SELECT claimed_by AS user_id FROM agent_claims WHERE lower(agent_name) = ?1
         ) ORDER BY user_id",
    )
    .bind(&name)
    .fetch_all(db)
    .await?)
}

fn push_priority(kind: ChatPushKind) -> i64 {
    match kind {
        ChatPushKind::Ping => 3,
        ChatPushKind::Dm => 2,
        ChatPushKind::Room => 1,
    }
}

fn add_recipient(
    out: &mut std::collections::HashMap<String, (i64, ChatPushKind)>,
    member: String,
    user_id: i64,
    kind: ChatPushKind,
) {
    match out.get(&member) {
        Some((_, existing)) if push_priority(*existing) >= push_priority(kind) => {}
        _ => {
            out.insert(member, (user_id, kind));
        }
    }
}

fn mentions(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for ch in body.chars() {
        if ch == '@' {
            current = Some(String::new());
        } else if let Some(buf) = current.as_mut() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                buf.push(ch.to_ascii_lowercase());
            } else {
                let token = std::mem::take(buf);
                if !token.is_empty() {
                    out.push(token);
                }
                current = None;
            }
        }
    }
    if let Some(token) = current {
        if !token.is_empty() {
            out.push(token);
        }
    }
    out.dedup();
    out
}

/// Build the push inbox for a freshly posted message.
pub async fn create_chat_pushes(
    db: &SqlitePool,
    room: &ChatRoom,
    message: &ChatMessage,
    author_member: &str,
) -> AppResult<()> {
    let mut want: std::collections::HashMap<String, (i64, ChatPushKind)> =
        std::collections::HashMap::new();

    if room.kind == ChatRoomKind::Dm {
        for member in &room.members {
            if member == author_member {
                continue;
            }
            if let Some(agent) = member.strip_prefix("agent:") {
                for owner in agent_owners(db, agent).await? {
                    add_recipient(&mut want, member.clone(), owner, ChatPushKind::Dm);
                }
            } else if let Some(login) = member.strip_prefix("human:") {
                if let Some(id) = find_user_by_login(db, login).await? {
                    add_recipient(&mut want, member.clone(), id, ChatPushKind::Dm);
                }
            }
        }
    } else {
        let subs: Vec<(i64, String)> = sqlx::query_as(
            "SELECT user_id, member FROM chat_push_subs WHERE room_id = ?1",
        )
        .bind(room.id)
        .fetch_all(db)
        .await?;
        for (user_id, member) in subs {
            if member != author_member {
                add_recipient(&mut want, member, user_id, ChatPushKind::Room);
            }
        }
    }

    for token in mentions(&message.body) {
        let agents: Vec<(i64, String)> = sqlx::query_as(
            "SELECT DISTINCT user_id, lower(agent_name) FROM (
                SELECT user_id, agent_name FROM agent_profiles
                 WHERE replace(lower(agent_name), ' ', '-') = ?1
                UNION
                SELECT claimed_by AS user_id, lower(agent_name) AS agent_name FROM agent_claims
                 WHERE replace(lower(agent_name), ' ', '-') = ?1
             )",
        )
        .bind(&token)
        .fetch_all(db)
        .await?;
        for (user_id, agent_name) in agents {
            let member = agent_member(&agent_name);
            if member != author_member {
                add_recipient(&mut want, member, user_id, ChatPushKind::Ping);
            }
        }
        if let Some(user_id) = find_user_by_login(db, &token).await? {
            let member = human_member(&token);
            if member != author_member {
                add_recipient(&mut want, member, user_id, ChatPushKind::Ping);
            }
        }
    }

    for (member, (user_id, kind)) in want {
        sqlx::query(
            "INSERT OR IGNORE INTO chat_pushes
                (user_id, member, room_id, message_id, kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(user_id)
        .bind(&member)
        .bind(room.id)
        .bind(message.id)
        .bind(kind.as_str())
        .bind(now_rfc3339())
        .execute(db)
        .await?;
    }
    Ok(())
}

pub async fn list_chat_pushes(
    db: &SqlitePool,
    member: &str,
    after: i64,
    limit: i64,
) -> AppResult<Vec<breadboard_core::ChatPush>> {
    #[derive(FromRow)]
    struct PushRow {
        id: i64,
        kind: String,
        room_id: i64,
        room_name: Option<String>,
        message_id: i64,
    }
    let rows: Vec<PushRow> = sqlx::query_as(
        "SELECT p.id, p.kind, p.room_id, r.name AS room_name, p.message_id
           FROM chat_pushes p
           JOIN chat_rooms r ON r.id = p.room_id
           JOIN chat_messages m ON m.id = p.message_id
          WHERE p.member = ?1 AND p.id > ?2
            AND m.deleted_at IS NULL AND m.compacted_at IS NULL
          ORDER BY p.id ASC LIMIT ?3",
    )
    .bind(member)
    .bind(after)
    .bind(limit)
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.message_id).collect();
    let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT id, room_id, author_member, author_display, author_kind, body, created_at, message_kind
           FROM chat_messages WHERE id IN (",
    );
    let mut sep = qb.separated(", ");
    for id in &ids {
        sep.push_bind(id);
    }
    qb.push(")");
    let message_rows: Vec<ChatMessageRow> = qb.build_query_as().fetch_all(db).await?;
    let messages = hydrate_messages(db, message_rows).await?;
    let by_id: std::collections::HashMap<i64, ChatMessage> =
        messages.into_iter().map(|m| (m.id, m)).collect();

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let message = by_id.get(&row.message_id)?.clone();
            Some(breadboard_core::ChatPush {
                id: row.id,
                kind: ChatPushKind::parse(&row.kind).unwrap_or(ChatPushKind::Room),
                room_id: row.room_id,
                room_name: row.room_name,
                message,
            })
        })
        .collect())
}

pub async fn ack_chat_pushes(db: &SqlitePool, member: &str, up_to: i64) -> AppResult<()> {
    sqlx::query("DELETE FROM chat_pushes WHERE member = ?1 AND id <= ?2")
        .bind(member)
        .bind(up_to)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn chat_is_member(db: &SqlitePool, room_id: i64, member: &str) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_room_members WHERE room_id = ?1 AND member = ?2",
    )
    .bind(room_id)
    .bind(member)
    .fetch_one(db)
    .await?;
    Ok(n > 0)
}

/// Return the DM room for two members, creating it the first time.
pub async fn find_or_create_dm(
    db: &SqlitePool,
    a: &str,
    b: &str,
) -> AppResult<ChatRoom> {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let key = format!("{a}|{b}");
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM chat_rooms WHERE dm_key = ?1")
        .bind(&key)
        .fetch_optional(db)
        .await?
    {
        return get_chat_room(db, id).await;
    }

    let mut tx = crate::db::begin_write(db).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_rooms (kind, dm_key, created_at) VALUES ('dm', ?1, ?2) RETURNING id",
    )
    .bind(&key)
    .bind(now_rfc3339())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
            AppError::Conflict("dm already exists".into())
        }
        other => AppError::Db(other),
    })?;
    for member in [a, b] {
        sqlx::query("INSERT INTO chat_room_members (room_id, member) VALUES (?1, ?2)")
            .bind(id)
            .bind(member)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    get_chat_room(db, id).await
}

pub async fn chat_messages(
    db: &SqlitePool,
    room_id: i64,
    after_id: i64,
    limit: i64,
) -> AppResult<Vec<ChatMessage>> {
    let rows: Vec<ChatMessageRow> = sqlx::query_as(
        "SELECT id, room_id, author_member, author_display, author_kind, body, created_at, message_kind
           FROM chat_messages
          WHERE room_id = ?1 AND id > ?2
            AND deleted_at IS NULL AND compacted_at IS NULL
          ORDER BY id ASC
          LIMIT ?3",
    )
    .bind(room_id)
    .bind(after_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    hydrate_messages(db, rows).await
}

#[allow(clippy::too_many_arguments)]
pub async fn post_chat_message(
    db: &SqlitePool,
    room_id: i64,
    author_member: &str,
    author_display: &str,
    author_kind: ChatAuthorKind,
    body: &str,
    uploads: &[i64],
    uploader_user_id: i64,
) -> AppResult<ChatMessage> {
    let mut tx = crate::db::begin_write(db).await?;
    let row: ChatMessageRow = sqlx::query_as(
        "INSERT INTO chat_messages
            (room_id, author_member, author_display, author_kind, body, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         RETURNING id, room_id, author_member, author_display, author_kind, body, created_at, message_kind",
    )
    .bind(room_id)
    .bind(author_member)
    .bind(author_display)
    .bind(author_kind.as_str())
    .bind(body)
    .bind(now_rfc3339())
    .fetch_one(&mut *tx)
    .await?;

    let mut seen = std::collections::HashSet::new();
    for upload in uploads {
        if !seen.insert(*upload) {
            continue;
        }
        let attached = sqlx::query(
            "UPDATE chat_uploads SET message_id = ?2
              WHERE id = ?1 AND uploader_user_id = ?3 AND message_id IS NULL",
        )
        .bind(upload)
        .bind(row.id)
        .bind(uploader_user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if attached == 0 {
            return Err(AppError::bad_request(
                "upload not found, already used, or not yours",
            ));
        }
    }
    tx.commit().await?;

    let mut message: ChatMessage = row.into();
    message.attachments = attachments_for(db, &[message.id])
        .await?
        .remove(&message.id)
        .unwrap_or_default();
    Ok(message)
}

/// One visible message by id, for authorization before delete.
pub async fn get_chat_message(
    db: &SqlitePool,
    id: i64,
) -> AppResult<Option<breadboard_core::ChatMessage>> {
    let row: Option<ChatMessageRow> = sqlx::query_as(
        "SELECT id, room_id, author_member, author_display, author_kind, body, created_at, message_kind
           FROM chat_messages
          WHERE id = ?1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    match row {
        Some(row) => Ok(hydrate_messages(db, vec![row]).await?.pop()),
        None => Ok(None),
    }
}

/// Soft-delete one message and drop any pushes that referenced it.
pub async fn delete_chat_message(
    db: &SqlitePool,
    id: i64,
    deleted_by: &str,
) -> AppResult<bool> {
    let mut tx = crate::db::begin_write(db).await?;
    let changed = sqlx::query(
        "UPDATE chat_messages SET deleted_at = ?1, deleted_by = ?2
          WHERE id = ?3 AND deleted_at IS NULL",
    )
    .bind(now_rfc3339())
    .bind(deleted_by)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(false);
    }
    sqlx::query("DELETE FROM chat_pushes WHERE message_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Insert a summary message and hide every visible message below it.
pub async fn compact_chat_room(
    db: &SqlitePool,
    room_id: i64,
    summary: &str,
) -> AppResult<breadboard_core::ChatMessage> {
    let mut tx = crate::db::begin_write(db).await?;
    let now = now_rfc3339();
    let row: ChatMessageRow = sqlx::query_as(
        "INSERT INTO chat_messages
            (room_id, author_member, author_display, author_kind, body, created_at, message_kind)
         VALUES (?1, 'system', 'system', 'human', ?2, ?3, 'summary')
         RETURNING id, room_id, author_member, author_display, author_kind, body, created_at, message_kind",
    )
    .bind(room_id)
    .bind(summary)
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE chat_messages SET compacted_at = ?1
          WHERE room_id = ?2 AND id < ?3
            AND deleted_at IS NULL AND compacted_at IS NULL",
    )
    .bind(&now)
    .bind(room_id)
    .bind(row.id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "DELETE FROM chat_pushes
          WHERE message_id IN (
            SELECT id FROM chat_messages WHERE room_id = ?1 AND compacted_at = ?2
          )",
    )
    .bind(room_id)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    let mut message: breadboard_core::ChatMessage = row.into();
    message.attachments = Vec::new();
    Ok(message)
}

pub struct StoredUpload {
    pub upload: breadboard_core::ChatUpload,
    pub path: String,
}

pub async fn insert_chat_upload(
    db: &SqlitePool,
    uploader_user_id: i64,
    filename: &str,
    mime: &str,
    size: i64,
    path: &str,
) -> AppResult<breadboard_core::ChatUpload> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_uploads (uploader_user_id, filename, mime, size, path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(uploader_user_id)
    .bind(filename)
    .bind(mime)
    .bind(size)
    .bind(path)
    .bind(now_rfc3339())
    .fetch_one(db)
    .await?;
    Ok(breadboard_core::ChatUpload {
        id,
        filename: filename.to_string(),
        mime: mime.to_string(),
        size,
    })
}

pub async fn get_chat_upload(db: &SqlitePool, id: i64) -> AppResult<StoredUpload> {
    let row: Option<(i64, String, String, i64, String)> = sqlx::query_as(
        "SELECT id, filename, mime, size, path FROM chat_uploads WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let (id, filename, mime, size, path) = row.ok_or(AppError::NotFound)?;
    Ok(StoredUpload {
        upload: breadboard_core::ChatUpload {
            id,
            filename,
            mime,
            size,
        },
        path,
    })
}

/// True when this user has ever claimed or named that agent.
pub async fn user_has_agent(db: &SqlitePool, user_id: i64, name: &str) -> AppResult<bool> {
    let name = name.trim().to_lowercase();
    let n: i64 = sqlx::query_scalar(
        "SELECT
           (SELECT COUNT(*) FROM agent_profiles WHERE user_id = ?1 AND agent_name = ?2) +
           (SELECT COUNT(*) FROM agent_claims WHERE claimed_by = ?1 AND lower(agent_name) = ?2)",
    )
    .bind(user_id)
    .bind(&name)
    .fetch_one(db)
    .await?;
    Ok(n > 0)
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
