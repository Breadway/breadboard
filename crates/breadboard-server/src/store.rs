//! Database access + row→domain mapping.
//!
//! Runtime-checked `sqlx` queries (no compile-time macro, so the workspace
//! builds without a live database). Enums are stored as the snake/kebab-case
//! strings from `breadboard-core`; timestamps as RFC3339 UTC text.

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use breadboard_core::{
    Comment, Cycle, Epic, ForgejoRef, HandoffNote, Issue, IssueLink, IssueSession, IssueType,
    LinkKind, Priority, Project, Status, SyncSource, User,
};
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::config::ClaimLimits;
use crate::dto::{
    AgentRegister, AgentRegistration, AgentRemoval, AgentRosterEntry, IssueFilter, IssuePatch,
    NewCycle, NewEpic, NewIssue, NewProject,
};
use crate::error::{AppError, AppResult};
use crate::names;

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

    let last_handoff = latest_handoff(db, r.id).await?;
    let mut issue = finish_issue(r, labels, link_rows);
    issue.last_handoff = last_handoff;
    Ok(issue)
}

/// Hydrate a page in two grouped queries instead of two per row. A list of N
/// issues costs 3 queries total (base select, labels, links), so the request
/// cost no longer grows with the page size (BB-126).
async fn hydrate_rows(db: &SqlitePool, rows: Vec<IssueRow>) -> AppResult<Vec<Issue>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();

    let mut labels_qb: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new("SELECT issue_id, label FROM issue_labels WHERE issue_id IN (");
    {
        let mut separated = labels_qb.separated(", ");
        for id in &ids {
            separated.push_bind(*id);
        }
    }
    labels_qb.push(") ORDER BY issue_id, label");
    let label_rows: Vec<(i64, String)> = labels_qb.build_query_as().fetch_all(db).await?;

    let mut links_qb: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(
        "SELECT issue_id, target_id, kind FROM issue_links WHERE issue_id IN (",
    );
    {
        let mut separated = links_qb.separated(", ");
        for id in &ids {
            separated.push_bind(*id);
        }
    }
    links_qb.push(") ORDER BY issue_id");
    let link_rows: Vec<(i64, i64, String)> = links_qb.build_query_as().fetch_all(db).await?;

    // Latest handoff per issue for the page, one grouped query. Ordered newest
    // first so the first row seen for an id wins.
    let mut handoff_qb: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(format!(
        "SELECT {} FROM handoff_notes h JOIN issues i ON i.id = h.issue_id WHERE h.issue_id IN (",
        HANDOFF_COLS
    ));
    {
        let mut separated = handoff_qb.separated(", ");
        for id in &ids {
            separated.push_bind(*id);
        }
    }
    handoff_qb.push(") ORDER BY h.created_at DESC, h.id DESC");
    let handoff_rows: Vec<HandoffRow> = handoff_qb.build_query_as().fetch_all(db).await?;
    let mut handoff_by_issue: std::collections::HashMap<i64, HandoffNote> =
        std::collections::HashMap::new();
    for row in handoff_rows {
        handoff_by_issue
            .entry(row.issue_id)
            .or_insert_with(|| row.into());
    }

    let mut labels_by_issue: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    for (issue_id, label) in label_rows {
        labels_by_issue.entry(issue_id).or_default().push(label);
    }
    let mut links_by_issue: std::collections::HashMap<i64, Vec<(i64, String)>> =
        std::collections::HashMap::new();
    for (issue_id, target_id, kind) in link_rows {
        links_by_issue
            .entry(issue_id)
            .or_default()
            .push((target_id, kind));
    }

    Ok(rows
        .into_iter()
        .map(|r| {
            let labels = labels_by_issue.remove(&r.id).unwrap_or_default();
            let links = links_by_issue.remove(&r.id).unwrap_or_default();
            let last_handoff = handoff_by_issue.remove(&r.id);
            let mut issue = finish_issue(r, labels, links);
            issue.last_handoff = last_handoff;
            issue
        })
        .collect())
}

fn finish_issue(r: IssueRow, labels: Vec<String>, link_rows: Vec<(i64, String)>) -> Issue {
    let links = link_rows
        .into_iter()
        .filter_map(|(target_id, kind)| {
            LinkKind::parse(&kind).map(|kind| IssueLink { kind, target_id })
        })
        .collect();

    Issue {
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
        last_handoff: None,
        forgejo_ref: match (r.forgejo_repo, r.forgejo_number) {
            (Some(repo), Some(number)) => Some(breadboard_core::ForgejoRef { repo, number }),
            _ => None,
        },
        milestone: r.milestone,
        created_at: parse_ts(&r.created_at),
        updated_at: parse_ts(&r.updated_at),
    }
}

/// Columns for a hydrated handoff note joined with its issue key.
const HANDOFF_COLS: &str = "h.id, h.issue_id, i.key AS key, h.session_id, \
    h.agent_name, h.mode, h.state, h.note, h.created_at";

#[derive(FromRow)]
struct HandoffRow {
    id: i64,
    issue_id: i64,
    key: String,
    session_id: Option<String>,
    agent_name: String,
    mode: String,
    state: String,
    note: String,
    created_at: String,
}

impl From<HandoffRow> for HandoffNote {
    fn from(r: HandoffRow) -> Self {
        HandoffNote {
            id: r.id,
            issue_id: r.issue_id,
            key: r.key,
            session_id: r.session_id,
            agent_name: r.agent_name,
            mode: ClaimMode::parse(&r.mode).unwrap_or(ClaimMode::Exclusive),
            state: AgentState::parse(&r.state).unwrap_or(AgentState::Implementing),
            note: r.note,
            created_at: parse_ts(&r.created_at),
        }
    }
}

/// Most recent handoff note for one issue, if any. This is what a resuming
/// session reads as context.
pub async fn latest_handoff(db: &SqlitePool, issue_id: i64) -> AppResult<Option<HandoffNote>> {
    let row: Option<HandoffRow> = sqlx::query_as(&format!(
        "SELECT {HANDOFF_COLS} FROM handoff_notes h JOIN issues i ON i.id = h.issue_id \
          WHERE h.issue_id = ?1 ORDER BY h.created_at DESC, h.id DESC LIMIT 1"
    ))
    .bind(issue_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

/// Handoff history for an issue, newest first.
pub async fn list_issue_handoffs(
    db: &SqlitePool,
    issue_id: i64,
    limit: i64,
) -> AppResult<Vec<HandoffNote>> {
    let rows: Vec<HandoffRow> = sqlx::query_as(&format!(
        "SELECT {HANDOFF_COLS} FROM handoff_notes h JOIN issues i ON i.id = h.issue_id \
          WHERE h.issue_id = ?1 ORDER BY h.created_at DESC, h.id DESC LIMIT ?2"
    ))
    .bind(issue_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[derive(FromRow)]
struct IssueSessionRow {
    issue_id: i64,
    key: String,
    title: String,
    status: String,
    session_id: String,
    agent_name: String,
    first_seen: String,
    last_seen: String,
    last_state: Option<String>,
}

const ISSUE_SESSION_COLS: &str = "s.issue_id, i.key AS key, i.title AS title, \
    i.status AS status, s.session_id, s.agent_name, s.first_seen, s.last_seen, s.last_state";

impl From<IssueSessionRow> for IssueSession {
    fn from(r: IssueSessionRow) -> Self {
        IssueSession {
            issue_id: r.issue_id,
            key: r.key,
            title: r.title,
            status: Status::parse(&r.status).unwrap_or(Status::Backlog),
            session_id: r.session_id,
            agent_name: r.agent_name,
            first_seen: parse_ts(&r.first_seen),
            last_seen: parse_ts(&r.last_seen),
            last_state: r.last_state.as_deref().and_then(AgentState::parse),
        }
    }
}

/// Sessions that touched one issue, most recent action first.
pub async fn list_issue_sessions(db: &SqlitePool, issue_id: i64) -> AppResult<Vec<IssueSession>> {
    let rows: Vec<IssueSessionRow> = sqlx::query_as(&format!(
        "SELECT {ISSUE_SESSION_COLS} FROM issue_sessions s JOIN issues i ON i.id = s.issue_id \
          WHERE s.issue_id = ?1 ORDER BY s.last_seen DESC, s.session_id"
    ))
    .bind(issue_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// The issue links for one session (issue -> sessions and back).
pub async fn list_session_issues(
    db: &SqlitePool,
    session_id: &str,
) -> AppResult<Vec<IssueSession>> {
    let rows: Vec<IssueSessionRow> = sqlx::query_as(&format!(
        "SELECT {ISSUE_SESSION_COLS} FROM issue_sessions s JOIN issues i ON i.id = s.issue_id \
          WHERE s.session_id = ?1 ORDER BY s.last_seen DESC, i.key"
    ))
    .bind(session_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Upsert a session link. `first_seen` is preserved on later heartbeats; the
/// release path deliberately calls this rather than deleting, so the link
/// outlives the claim.
pub async fn link_issue_session(
    db: &SqlitePool,
    issue_id: i64,
    session_id: &str,
    agent_name: &str,
    state: AgentState,
    at: &str,
) -> AppResult<()> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO issue_sessions
            (issue_id, session_id, agent_name, first_seen, last_seen, last_state)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5)
         ON CONFLICT(issue_id, session_id) DO UPDATE SET
            agent_name = excluded.agent_name,
            last_seen = excluded.last_seen,
            last_state = excluded.last_state",
    )
    .bind(issue_id)
    .bind(session_id)
    .bind(crate::names::normalize(agent_name))
    .bind(at)
    .bind(state.as_str())
    .execute(db)
    .await?;
    Ok(())
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

/// Default page size for `GET /api/issues`; keeps a browser tab or CLI from
/// pulling the whole history in one response.
pub const DEFAULT_ISSUE_PAGE: u32 = 500;
/// Hard ceiling on a single page.
pub const MAX_ISSUE_PAGE: u32 = 2000;

pub async fn list_issues(db: &SqlitePool, filter: &IssueFilter) -> AppResult<Vec<Issue>> {
    let limit = Some(
        filter
            .limit
            .unwrap_or(DEFAULT_ISSUE_PAGE)
            .clamp(1, MAX_ISSUE_PAGE),
    );
    list_issues_page(db, filter, limit).await
}

/// Every matching issue, ignoring `limit`. Only for internal callers that need
/// the full set (import title matching); HTTP callers page with `limit`/`offset`.
pub async fn list_issues_all(db: &SqlitePool, filter: &IssueFilter) -> AppResult<Vec<Issue>> {
    list_issues_page(db, filter, None).await
}

async fn list_issues_page(
    db: &SqlitePool,
    filter: &IssueFilter,
    limit: Option<u32>,
) -> AppResult<Vec<Issue>> {
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
    // `id` breaks updated_at ties so LIMIT/OFFSET pages cannot overlap or skip
    // a row when several issues share a timestamp.
    qb.push(" ORDER BY updated_at DESC, id DESC");

    if let Some(limit) = limit {
        qb.push(" LIMIT ").push_bind(i64::from(limit));
    }
    let offset = i64::from(filter.offset.unwrap_or(0));
    if offset > 0 {
        if limit.is_none() {
            // SQLite needs a LIMIT before OFFSET; -1 means no limit.
            qb.push(" LIMIT -1");
        }
        qb.push(" OFFSET ").push_bind(offset);
    }

    let rows: Vec<IssueRow> = qb.build_query_as().fetch_all(db).await?;
    hydrate_rows(db, rows).await
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

    // Review requests gate `done` the same way CI does: only an approved
    // request clears the card. A user-facing mutation is refused with a
    // conflict; the sync cascade leaves the card where it is so a Forgejo
    // close cannot bypass an outstanding review.
    let review_hold = patch.status == Some(Status::Done)
        && before.status != Status::Done
        && crate::reviews::has_outstanding(db, before.id).await?;
    if review_hold && source == SyncSource::Breadboard {
        let reviewer = crate::reviews::first_outstanding(db, before.id)
            .await?
            .map(|r| format!("{} ({})", r.reviewer, r.state.as_str()))
            .unwrap_or_else(|| "a reviewer".to_string());
        return Err(AppError::Conflict(format!(
            "{} has an outstanding review request from {}; resolve it before marking done",
            before.key, reviewer
        )));
    }

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
        if status != before.status && !(review_hold && status == Status::Done) {
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
    // A merged PR cannot complete a target that still owes a review. Hold it
    // in review, matching the `verify-before-done` cascade.
    let status = match status {
        Some(Status::Done) if crate::reviews::has_outstanding(db, target_id).await? => {
            Some(Status::Review)
        }
        other => other,
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

/// Local issues in mapped, enabled repos that never got their Forgejo link.
/// The reconcile loop retries these so a failed create writeback is durable
/// without a separate queue table (BB-132).
pub async fn list_unlinked_for_writeback(db: &SqlitePool, limit: i64) -> AppResult<Vec<Issue>> {
    let cols = "i.id, i.key, i.project_id, i.title, i.body, i.status, i.priority, i.type, \
        i.estimate, i.assignee_id, i.reporter_id, i.epic_id, i.cycle_id, i.forgejo_repo, \
        i.forgejo_number, i.milestone, i.created_at, i.updated_at";
    let rows: Vec<IssueRow> = sqlx::query_as(&format!(
        "SELECT {cols} FROM issues i \
         JOIN projects p ON p.id = i.project_id \
         JOIN repo_sync_state r ON r.forgejo_repo = p.forgejo_repo \
         WHERE i.forgejo_repo IS NULL AND p.forgejo_repo IS NOT NULL AND r.enabled = 1 \
         ORDER BY i.updated_at ASC LIMIT ?1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(hydrate(db, row).await?);
    }
    Ok(out)
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

/// Live exclusive claim usage plus the resolved orchestrator tree (BB-154).
///
/// The tree comes from `agent_sessions`: a name whose `parent_session_id`
/// matches another registered session's `session_id` rolls up under that
/// parent. An unlinked name (or a parent that is not a registered session) is
/// its own root, which matches the roster's resolution rules.
#[derive(Debug, Default)]
pub struct ClaimUsage {
    /// Lowercased agent name -> live exclusive claims.
    pub counts: HashMap<String, i64>,
    /// Lowercased agent name -> orchestrator tree root name.
    pub roots: HashMap<String, String>,
    /// Root name -> live exclusive claims across the whole tree.
    pub totals: HashMap<String, i64>,
}

impl ClaimUsage {
    pub fn root_of(&self, name: &str) -> String {
        let key = name.trim().to_lowercase();
        self.roots.get(&key).cloned().unwrap_or(key)
    }

    pub fn own_count(&self, name: &str) -> i64 {
        self.counts
            .get(&name.trim().to_lowercase())
            .copied()
            .unwrap_or(0)
    }

    pub fn tree_count(&self, name: &str) -> i64 {
        self.totals.get(&self.root_of(name)).copied().unwrap_or(0)
    }
}

/// Walk up `parent_session_id` until a name has no registered parent. A
/// repeated name in one chain is a cycle and stops the walk at the repeat.
fn tree_root(name: &str, parents: &HashMap<String, String>) -> String {
    let mut current = name.to_string();
    let mut seen = HashSet::new();
    while let Some(parent) = parents.get(&current) {
        if !seen.insert(current.clone()) {
            break;
        }
        current = parent.clone();
    }
    current
}

/// Snapshot exclusive claim counts and the orchestrator forest in one pass.
pub async fn claim_usage(db: &SqlitePool) -> AppResult<ClaimUsage> {
    let count_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT lower(agent_name) AS name, COUNT(*) AS n
           FROM agent_claims
          WHERE mode = 'exclusive'
          GROUP BY lower(agent_name)",
    )
    .fetch_all(db)
    .await?;
    let counts: HashMap<String, i64> = count_rows.into_iter().collect();

    let sessions: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT lower(agent_name), session_id, parent_session_id FROM agent_sessions",
    )
    .fetch_all(db)
    .await?;
    let by_session: HashMap<String, String> = sessions
        .iter()
        .filter_map(|(name, session, _)| session.clone().map(|id| (id, name.clone())))
        .collect();
    let mut parents: HashMap<String, String> = HashMap::new();
    for (name, _, parent_session) in &sessions {
        if let Some(parent_name) = parent_session.as_ref().and_then(|id| by_session.get(id)) {
            if parent_name != name {
                parents.insert(name.clone(), parent_name.clone());
            }
        }
    }

    let mut roots: HashMap<String, String> = HashMap::new();
    // A name can appear in both `counts` and `sessions`, so resolve each name
    // once instead of summing its claims once per source.
    for name in sessions
        .iter()
        .map(|(name, _, _)| name)
        .chain(counts.keys())
    {
        if !roots.contains_key(name) {
            roots.insert(name.clone(), tree_root(name, &parents));
        }
    }
    let mut totals: HashMap<String, i64> = HashMap::new();
    for (name, n) in &counts {
        let root = roots.get(name).cloned().unwrap_or_else(|| name.clone());
        *totals.entry(root).or_insert(0) += n;
    }

    Ok(ClaimUsage {
        counts,
        roots,
        totals,
    })
}

/// Refuse a new exclusive claim when either live budget is already full.
/// Intents never call this: only exclusive claims are capped.
pub async fn ensure_claim_budget(
    db: &SqlitePool,
    agent_name: &str,
    limits: &ClaimLimits,
) -> AppResult<()> {
    let usage = claim_usage(db).await?;
    let name = agent_name.trim().to_lowercase();
    let own = usage.own_count(&name);
    if own >= limits.per_agent {
        return Err(AppError::Conflict(format!(
            "'{name}' already holds {own} live exclusive claims (limit {} per agent); \
             release a claim before claiming another",
            limits.per_agent
        )));
    }
    let root = usage.root_of(&name);
    let total = usage.tree_count(&name);
    if total >= limits.per_orchestrator {
        return Err(AppError::Conflict(format!(
            "orchestrator '{root}' and its subagents already hold {total} live exclusive claims \
             (limit {} per orchestrator); release a claim before claiming another",
            limits.per_orchestrator
        )));
    }
    Ok(())
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
    limits: &ClaimLimits,
) -> AppResult<AgentClaim> {
    input.validate().map_err(AppError::BadRequest)?;
    let mode = input.mode.unwrap_or(ClaimMode::Exclusive);
    let ts = now_rfc3339();
    let state = input.state.unwrap_or(AgentState::Implementing);
    // Usage stats: a claim without a session id cannot be tied to the tokens a
    // session spent on it. If the caller did not send one, reuse the session id
    // this agent already registered on the roster.
    let session_id = match input
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(id.to_string()),
        None => crate::usage::session_for_agent(db, claimed_by, &input.agent_name).await?,
    };

    if mode == ClaimMode::Exclusive && has_exclusive_claim(db, issue.id).await? {
        return Err(AppError::Conflict("issue is already claimed".into()));
    }

    // Budgets cover new exclusive work and promoting a queued intent into an
    // exclusive claim: both arrive here as `mode = exclusive`. The intent is
    // still in place, so a refusal leaves the soft reservation untouched.
    if mode == ClaimMode::Exclusive {
        ensure_claim_budget(db, &input.agent_name, limits).await?;
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
                session_id: session_id.clone().map(Some),
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
            crate::usage::link_session_issue(db, session_id.as_deref(), issue.id).await?;
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
    .bind(session_id.clone())
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
            crate::usage::link_session_issue(db, session_id.as_deref(), issue.id).await?;
            let claim = get_claim_by_id(db, done.last_insert_rowid())
                .await?
                .ok_or(AppError::NotFound)?;
            if let Some(session) = claim.session_id.as_deref() {
                link_issue_session(db, issue.id, session, &claim.agent_name, claim.state, &ts)
                    .await?;
            }
            // Timeline: a new claim (not an intent re-claim, which returns
            // through update_claim_row above).
            crate::agent_events::record_best_effort(
                db,
                crate::agent_events::AgentEventInsert::new(
                    &claim.agent_name,
                    breadboard_core::AgentEventKind::Claimed,
                )
                .issue(issue.id, &issue.key)
                .summary(format!("Claimed {} ({})", issue.key, claim.mode.as_str()))
                .detail(format!("state: {}", claim.state.as_str()))
                .actor(Some(claimed_by)),
            )
            .await;
            if claim.pr_url.is_some() {
                crate::agent_events::record_best_effort(
                    db,
                    crate::agent_events::AgentEventInsert::new(
                        &claim.agent_name,
                        breadboard_core::AgentEventKind::PrOpened,
                    )
                    .issue(issue.id, &issue.key)
                    .pr(claim.pr_url.as_deref(), claim.pr_number)
                    .summary(match &claim.pr_url {
                        Some(url) => format!("Opened {url}"),
                        None => "Opened a pull request".to_string(),
                    })
                    .actor(Some(claimed_by)),
                )
                .await;
            }
            Ok(claim)
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
                        session_id: session_id.clone().map(Some),
                        note: input.note.clone().map(Some),
                        branch: input.branch.clone().map(Some),
                        worktree: input.worktree.clone().map(Some),
                        repo: input.repo.clone().map(Some),
                        pr_url: input.pr_url.clone().map(Some),
                        pr_number: input.pr_number.map(Some),
                        head_sha: input.head_sha.clone().map(Some),
                        message: input.message.clone().map(Some),
                    };
                    crate::usage::link_session_issue(db, session_id.as_deref(), issue.id).await?;
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
    // Snapshot the fields the timeline reports as transitions. Read inside
    // the write transaction so the comparison cannot race another writer.
    let (before_state, before_pr_url): (String, Option<String>) =
        sqlx::query_as("SELECT state, pr_url FROM agent_claims WHERE id = ?1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;

    if let Some(agent_name) = &input.agent_name {
        let name = agent_name.trim();
        if name.is_empty() {
            return Err(AppError::bad_request("agent_name must not be empty"));
        }
        if name.len() > 64 {
            return Err(AppError::bad_request(
                "agent_name must be 64 chars or fewer",
            ));
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
                sqlx::query(concat!(
                    "UPDATE agent_claims SET ",
                    $col,
                    " = ?2 WHERE id = ?1"
                ))
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
    let (issue_id, issue_key, claimed_by, agent_name): (i64, String, i64, String) = sqlx::query_as(
        "SELECT c.issue_id, i.key, c.claimed_by, c.agent_name
               FROM agent_claims c JOIN issues i ON i.id = c.issue_id
              WHERE c.id = ?1",
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
        let normalized =
            breadboard_core::dto::normalize_agent_message(message.as_deref().unwrap_or_default())
                .map_err(AppError::BadRequest)?;
        set_agent_message(db, claimed_by, &agent_name, normalized.as_deref()).await?;
    }

    let claim = get_claim_by_id(db, id).await?.ok_or(AppError::NotFound)?;

    if let Some(session) = claim.session_id.as_deref() {
        link_issue_session(
            db,
            claim.issue_id,
            session,
            &claim.agent_name,
            claim.state,
            ts,
        )
        .await?;
    }

    if claim.state.as_str() != before_state {
        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &claim.agent_name,
                breadboard_core::AgentEventKind::StateChange,
            )
            .issue(issue_id, &issue_key)
            .summary(format!(
                "{}: {} -> {}",
                issue_key,
                before_state,
                claim.state.as_str()
            ))
            .actor(Some(claimed_by)),
        )
        .await;
    }
    if let Some(url) = &claim.pr_url {
        if before_pr_url.as_deref() != Some(url.as_str()) {
            crate::agent_events::record_best_effort(
                db,
                crate::agent_events::AgentEventInsert::new(
                    &claim.agent_name,
                    breadboard_core::AgentEventKind::PrOpened,
                )
                .issue(issue_id, &issue_key)
                .pr(Some(url.as_str()), claim.pr_number)
                .summary(format!("Opened {url}"))
                .actor(Some(claimed_by)),
            )
            .await;
        }
    }
    Ok(claim)
}

pub async fn update_claim(
    db: &SqlitePool,
    claim_id: i64,
    input: &crate::dto::ClaimUpdate,
) -> AppResult<AgentClaim> {
    update_claim_row(db, claim_id, input, &now_rfc3339()).await
}

/// Exact message shown when a release omits the required handoff note. Kept
/// as one string so the REST API, CLI and MCP all surface the same rule.
pub const RELEASE_NOTE_REQUIRED: &str = "release requires a handoff note: pass note=\"done: ...; next: ...; risky: ...\" (one line, 500 characters max). Use no_note=true only when the claim never held work: an intent claim, or an exclusive claim still in planned state with no branch, worktree, repo, PR or head commit.";

/// Exact message when `no_note` is used on a claim that did hold work.
pub const NO_NOTE_NOT_ALLOWED: &str = "no_note is only allowed for a claim that never held work: an intent claim, or an exclusive claim still in planned state with no branch, worktree, repo, PR or head commit. This claim held work, so it needs a handoff note covering what is done, what is next and what is risky.";

fn normalize_handoff_note(raw: &str) -> Option<String> {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!one_line.is_empty()).then_some(one_line)
}

fn claim_never_held_work(claim: &AgentClaim) -> bool {
    if claim.mode == ClaimMode::Intent {
        return true;
    }
    claim.state == AgentState::Planned
        && claim.branch.is_none()
        && claim.worktree.is_none()
        && claim.repo.is_none()
        && claim.pr_url.is_none()
        && claim.pr_number.is_none()
        && claim.head_sha.is_none()
}

/// Release one claim by id, enforcing the handoff-note rule and preserving
/// the session link. Returns the claim that was removed.
///
/// `note` is the required one-line handoff. `no_note` is an explicit opt-out
/// for a claim that never held work; it is never inferred from a missing note.
/// The handoff row, the session's last-seen refresh and the claim deletion
/// commit together, so a refused or failed note cannot silently drop the claim.
pub async fn release_claim_with_handoff(
    db: &SqlitePool,
    id: i64,
    actor_id: Option<i64>,
    note: Option<&str>,
    no_note: bool,
) -> AppResult<AgentClaim> {
    let claim = get_claim_by_id(db, id).await?.ok_or(AppError::NotFound)?;

    let normalized = note.and_then(normalize_handoff_note);
    if let Some(text) = &normalized {
        if text.chars().count() > 500 {
            return Err(AppError::bad_request(
                "handoff note must be 500 characters or fewer (one line: what is done, what is next, what is risky)",
            ));
        }
    } else if !no_note {
        return Err(AppError::BadRequest(RELEASE_NOTE_REQUIRED.into()));
    } else if !claim_never_held_work(&claim) {
        return Err(AppError::BadRequest(NO_NOTE_NOT_ALLOWED.into()));
    }

    let ts = now_rfc3339();
    let mut tx = crate::db::begin_write(db).await?;
    if let Some(text) = &normalized {
        sqlx::query(
            "INSERT INTO handoff_notes
                (issue_id, session_id, agent_name, mode, state, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(claim.issue_id)
        .bind(claim.session_id.as_deref())
        .bind(&claim.agent_name)
        .bind(claim.mode.as_str())
        .bind(claim.state.as_str())
        .bind(text)
        .bind(&ts)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(session) = claim.session_id.as_deref() {
        // Preserve the linkage: refresh rather than delete, so the issue still
        // lists every session that touched it after the claim is gone.
        sqlx::query(
            "UPDATE issue_sessions SET last_seen = ?3, last_state = ?4
              WHERE issue_id = ?1 AND session_id = ?2",
        )
        .bind(claim.issue_id)
        .bind(session)
        .bind(&ts)
        .bind(claim.state.as_str())
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM agent_claims WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if let Some(text) = &normalized {
        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &claim.agent_name,
                breadboard_core::AgentEventKind::Handoff,
            )
            .issue(claim.issue_id, &claim.key)
            .summary(text.clone())
            .detail(format!(
                "released {} ({}) at state {}",
                claim.key,
                claim.mode.as_str(),
                claim.state.as_str()
            ))
            .actor(actor_id),
        )
        .await;
    }
    crate::agent_events::record_best_effort(
        db,
        crate::agent_events::AgentEventInsert::new(
            &claim.agent_name,
            breadboard_core::AgentEventKind::Released,
        )
        .issue(claim.issue_id, &claim.key)
        .summary(format!("Released {} ({})", claim.key, claim.mode.as_str()))
        .actor(actor_id),
    )
    .await;
    Ok(claim)
}

pub async fn refresh_claim_board_status(
    db: &SqlitePool,
    issue_id: i64,
    status: Status,
) -> AppResult<()> {
    sqlx::query("UPDATE agent_claims SET board_status = ?2, updated_at = ?3 WHERE issue_id = ?1")
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

// ---------------------------------------------------------------------------
// agent roster: sessions, remove/pause, rename
// ---------------------------------------------------------------------------

/// A session with no heartbeat for longer than this is stale on the roster.
/// The claim grid keeps its own 5-minute threshold; this is only the session
/// liveness signal the human asked for.
pub const AGENT_LIVE_SECS: i64 = 120;

#[derive(FromRow)]
struct AgentSessionRow {
    agent_name: String,
    session_id: Option<String>,
    parent_session_id: Option<String>,
    host: Option<String>,
    pid: Option<i64>,
    note: Option<String>,
    user_id: Option<i64>,
    login: Option<String>,
    first_seen: String,
    last_seen: String,
    paused: i64,
    paused_by: Option<i64>,
    paused_by_login: Option<String>,
    paused_at: Option<String>,
}

#[derive(FromRow)]
struct AgentProfileRow {
    agent_name: String,
    message: String,
    user_id: i64,
    login: String,
}

#[derive(FromRow)]
struct AgentHiddenRow {
    agent_name: String,
    hidden_by: Option<i64>,
    hidden_by_login: Option<String>,
    hidden_at: String,
}

const AGENT_SESSION_COLS: &str = "s.agent_name, s.session_id, s.parent_session_id, s.host, s.pid, \
    s.note, s.user_id, u.login AS login, s.first_seen, s.last_seen, s.paused, s.paused_by, \
    p.login AS paused_by_login, s.paused_at";

fn agent_session_join() -> String {
    format!(
        "SELECT {AGENT_SESSION_COLS} FROM agent_sessions s \
         LEFT JOIN users u ON u.id = s.user_id \
         LEFT JOIN users p ON p.id = s.paused_by"
    )
}

fn empty_roster_entry(name: &str) -> AgentRosterEntry {
    AgentRosterEntry {
        agent_name: name.to_string(),
        session_id: None,
        host: None,
        pid: None,
        note: None,
        first_seen: None,
        last_seen: None,
        live: false,
        paused: false,
        status: "stale".to_string(),
        message: None,
        state: None,
        claim_count: 0,
        exclusive_count: 0,
        intent_count: 0,
        exclusive_limit: 0,
        orchestrator_exclusive_count: 0,
        orchestrator_exclusive_limit: 0,
        user_id: None,
        login: None,
        paused_by: None,
        paused_by_login: None,
        paused_at: None,
        hidden: false,
        hidden_by: None,
        hidden_by_login: None,
        hidden_at: None,
        parent_session_id: None,
        orchestrator: None,
        orchestrator_session_id: None,
    }
}

fn validate_agent_name(name: &str) -> AppResult<()> {
    if name.is_empty() {
        return Err(AppError::bad_request("agent_name must not be empty"));
    }
    if name.chars().count() > 64 {
        return Err(AppError::bad_request(
            "agent_name must be 64 chars or fewer",
        ));
    }
    Ok(())
}

/// Names already visible anywhere on the roster, lowercased.
pub async fn known_agent_names(db: &SqlitePool) -> AppResult<HashSet<String>> {
    let mut out = HashSet::new();
    let sessions: Vec<String> = sqlx::query_scalar("SELECT lower(agent_name) FROM agent_sessions")
        .fetch_all(db)
        .await?;
    let profiles: Vec<String> = sqlx::query_scalar("SELECT lower(agent_name) FROM agent_profiles")
        .fetch_all(db)
        .await?;
    let claims: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT lower(agent_name) FROM agent_claims")
            .fetch_all(db)
            .await?;
    out.extend(sessions);
    out.extend(profiles);
    out.extend(claims);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn upsert_agent_session(
    db: &SqlitePool,
    user_id: i64,
    name: &str,
    session_id: Option<&str>,
    parent_session_id: Option<&str>,
    host: Option<&str>,
    pid: Option<i64>,
    note: Option<&str>,
    ts: &str,
) -> AppResult<bool> {
    // The conflict guard is atomic: the UPDATE only applies when the caller
    // owns the row, does not identify itself (older clients), or the holder
    // has stopped heartbeating past the live window. A false WHERE clause
    // leaves the row untouched and rows_affected() == 0, which callers turn
    // into a name-taken refusal.
    let stale_before = (Utc::now() - chrono::Duration::seconds(AGENT_LIVE_SECS)).to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO agent_sessions
            (agent_name, session_id, parent_session_id, host, pid, note, user_id,
             first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(agent_name) DO UPDATE SET
            session_id         = COALESCE(excluded.session_id, agent_sessions.session_id),
            parent_session_id  = COALESCE(excluded.parent_session_id, agent_sessions.parent_session_id),
            host               = COALESCE(excluded.host, agent_sessions.host),
            pid                = COALESCE(excluded.pid, agent_sessions.pid),
            note               = COALESCE(excluded.note, agent_sessions.note),
            user_id            = COALESCE(agent_sessions.user_id, excluded.user_id),
            last_seen          = excluded.last_seen
         WHERE ?2 IS NULL
            OR agent_sessions.session_id IS NULL
            OR agent_sessions.session_id IS ?2
            OR julianday(agent_sessions.last_seen) < julianday(?9)",
    )
    .bind(name)
    .bind(session_id)
    .bind(parent_session_id)
    .bind(host)
    .bind(pid)
    .bind(note)
    .bind(user_id)
    .bind(ts)
    .bind(&stale_before)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Which session currently holds a live name, if any. Used to explain a
/// refusal rather than leaving the caller with a bare 409.
async fn live_name_holder(db: &SqlitePool, name: &str) -> AppResult<Option<String>> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT session_id, last_seen FROM agent_sessions WHERE agent_name = ?1")
            .bind(name)
            .fetch_optional(db)
            .await?;
    let Some((session_id, last_seen)) = row else {
        return Ok(None);
    };
    if (Utc::now() - parse_ts(&last_seen)).num_seconds() < AGENT_LIVE_SECS {
        Ok(session_id)
    } else {
        Ok(None)
    }
}

/// Build the 409 used by registration, claiming and chat when a live name is
/// already held by a different session. Registration gets curated
/// alternatives in the same response so the caller can rename before claiming.
async fn agent_name_taken(db: &SqlitePool, name: &str) -> AppError {
    let holder = live_name_holder(db, name).await.ok().flatten();
    let alternatives = match known_agent_names(db).await {
        Ok(known) => names::pick_names(&known, 3),
        Err(_) => Vec::new(),
    };
    let message = match holder.as_deref() {
        Some(session) => format!(
            "agent name '{name}' is live on session {session}; choose another name{}",
            alternatives_suffix(&alternatives)
        ),
        None => format!(
            "agent name '{name}' is live on another session; choose another name{}",
            alternatives_suffix(&alternatives)
        ),
    };
    AppError::AgentNameTaken {
        message,
        agent_name: name.to_string(),
        session_id: holder,
        alternatives,
    }
}

fn alternatives_suffix(alternatives: &[String]) -> String {
    if alternatives.is_empty() {
        String::new()
    } else {
        format!(" (alternatives: {})", alternatives.join(", "))
    }
}

/// Register or refresh one session. No `agent_name` assigns a unique curated
/// name; the response also carries a few alternatives.
pub async fn register_agent(
    db: &SqlitePool,
    user_id: i64,
    input: &AgentRegister,
    limits: &ClaimLimits,
) -> AppResult<AgentRegistration> {
    let explicit = input
        .agent_name
        .as_deref()
        .map(names::normalize)
        .filter(|name| !name.is_empty());

    let (name, mut alternatives) = match explicit {
        Some(name) => (name, Vec::new()),
        None => {
            let known = known_agent_names(db).await?;
            let picks = names::pick_names(&known, 4);
            let mut picks = picks.into_iter();
            let name = picks.next().ok_or_else(|| {
                AppError::Other(anyhow::anyhow!("no curated agent names are available"))
            })?;
            (name, picks.collect())
        }
    };
    validate_agent_name(&name)?;
    // A registration that inserts the row is the agent's first appearance;
    // refreshing an existing session is not an event worth logging.
    let first_registration: bool =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_sessions WHERE agent_name = ?1")
            .bind(&name)
            .fetch_one(db)
            .await?
            == 0;

    // Prefer the exact parent session id from the plugin. If only an
    // orchestrator name is known, resolve that name to its registered session;
    // an unknown name is ignored rather than failing registration.
    let parent_session_id = match input
        .parent_session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(id.to_string()),
        None => match input
            .orchestrator
            .as_deref()
            .map(names::normalize)
            .filter(|name| !name.is_empty())
        {
            Some(orchestrator) => sqlx::query_scalar(
                "SELECT session_id FROM agent_sessions
                  WHERE agent_name = ?1 AND session_id IS NOT NULL",
            )
            .bind(&orchestrator)
            .fetch_optional(db)
            .await?
            .flatten(),
            None => None,
        },
    };

    let ts = now_rfc3339();
    let registered = upsert_agent_session(
        db,
        user_id,
        &name,
        input.session_id.as_deref(),
        parent_session_id.as_deref(),
        input.host.as_deref(),
        input.pid,
        input.note.as_deref(),
        &ts,
    )
    .await?;
    if !registered {
        return Err(agent_name_taken(db, &name).await);
    }

    if first_registration {
        let detail = match (
            input.session_id.as_deref(),
            input.host.as_deref(),
            input.pid,
        ) {
            (Some(session), Some(host), Some(pid)) => {
                Some(format!("session {session} on {host} (pid {pid})"))
            }
            (Some(session), Some(host), None) => Some(format!("session {session} on {host}")),
            (Some(session), None, _) => Some(format!("session {session}")),
            (None, Some(host), Some(pid)) => Some(format!("on {host} (pid {pid})")),
            (None, Some(host), None) => Some(format!("on {host}")),
            _ => None,
        };
        let mut event = crate::agent_events::AgentEventInsert::new(
            &name,
            breadboard_core::AgentEventKind::Registered,
        )
        .summary(match input.host.as_deref() {
            Some(host) => format!("Registered on {host}"),
            None => "Registered".to_string(),
        })
        .actor(Some(user_id));
        if let Some(detail) = detail {
            event = event.detail(detail);
        }
        crate::agent_events::record_best_effort(db, event).await;
    }

    if alternatives.is_empty() {
        let known = known_agent_names(db).await?;
        alternatives = names::pick_names(&known, 3);
    }

    let entry = roster_entry(db, &name, limits)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(AgentRegistration {
        agent_name: name,
        alternatives,
        live: entry.live,
        paused: entry.paused,
        session_id: entry.session_id,
        parent_session_id: entry.parent_session_id,
        orchestrator: entry.orchestrator,
        orchestrator_session_id: entry.orchestrator_session_id,
    })
}

/// Auto-register an unknown name. Every agent-scoped write goes through this:
/// the human wants new sessions listed, not refused. Pause does not block
/// heartbeats or chat here; it blocks new claims through
/// [`ensure_agent_can_claim`].
pub async fn ensure_agent_writable(
    db: &SqlitePool,
    user_id: i64,
    raw_name: &str,
    session_id: Option<&str>,
) -> AppResult<String> {
    let name = names::normalize(raw_name);
    validate_agent_name(&name)?;

    let registered = upsert_agent_session(
        db,
        user_id,
        &name,
        session_id,
        None,
        None,
        None,
        None,
        &now_rfc3339(),
    )
    .await?;
    if !registered {
        return Err(agent_name_taken(db, &name).await);
    }
    Ok(name)
}

/// Gate for taking new work. A paused agent keeps its existing claims but may
/// not claim anything new until it is resumed.
pub async fn ensure_agent_can_claim(
    db: &SqlitePool,
    user_id: i64,
    raw_name: &str,
    session_id: Option<&str>,
) -> AppResult<String> {
    let name = ensure_agent_writable(db, user_id, raw_name, session_id).await?;
    let paused: Option<i64> =
        sqlx::query_scalar("SELECT paused FROM agent_sessions WHERE agent_name = ?1")
            .bind(&name)
            .fetch_optional(db)
            .await?;
    if paused == Some(1) {
        return Err(AppError::forbidden_msg(format!(
            "agent '{name}' is paused; resume it before claiming new work"
        )));
    }
    Ok(name)
}

/// Remove a name completely: release every claim it holds, delete its session
/// row, profile, hidden flag and chat notification state, and record one
/// server-side audit row. Unknown names are a 404 and never create a row.
pub async fn remove_agent(
    db: &SqlitePool,
    user_id: i64,
    raw_name: &str,
) -> AppResult<AgentRemoval> {
    let name = names::normalize(raw_name);
    validate_agent_name(&name)?;

    let known = known_agent_names(db).await?;
    if !known.contains(&name) {
        return Err(AppError::NotFound);
    }

    let ts = now_rfc3339();
    let member = format!("agent:{name}");
    let mut tx = crate::db::begin_write(db).await?;

    let claims_released = sqlx::query("DELETE FROM agent_claims WHERE lower(agent_name) = ?1")
        .bind(&name)
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;
    let profile_removed = sqlx::query("DELETE FROM agent_profiles WHERE lower(agent_name) = ?1")
        .bind(&name)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    sqlx::query("DELETE FROM agent_hidden WHERE lower(agent_name) = ?1")
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM chat_pushes WHERE member = ?1")
        .bind(&member)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM chat_push_subs WHERE member = ?1")
        .bind(&member)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM chat_reads WHERE member = ?1")
        .bind(&member)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO agent_removals
            (agent_name, removed_by, removed_at, reason, claims_released, profile_removed)
         VALUES (?1, ?2, ?3, 'kill', ?4, ?5)",
    )
    .bind(&name)
    .bind(user_id)
    .bind(&ts)
    .bind(claims_released)
    .bind(i64::from(profile_removed))
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM agent_sessions WHERE agent_name = ?1")
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // One action, one row: a mass removal is not a release per claim.
    crate::agent_events::record_best_effort(
        db,
        crate::agent_events::AgentEventInsert::new(&name, breadboard_core::AgentEventKind::Removed)
            .summary(format!("Removed; {claims_released} claim(s) released"))
            .detail(if profile_removed {
                "profile removed".to_string()
            } else {
                "profile kept".to_string()
            })
            .actor(Some(user_id)),
    )
    .await;

    Ok(AgentRemoval {
        agent_name: name,
        removed: true,
        claims_released,
        profile_removed,
    })
}

/// Pause a known name. Claims, profile and session row stay; new claims are
/// refused and pending chat pushes are dropped so an injector cannot wake it.
pub async fn pause_agent(
    db: &SqlitePool,
    user_id: i64,
    raw_name: &str,
    limits: &ClaimLimits,
) -> AppResult<(AgentRosterEntry, bool)> {
    let name = names::normalize(raw_name);
    validate_agent_name(&name)?;
    let known = known_agent_names(db).await?;
    if !known.contains(&name) {
        return Err(AppError::NotFound);
    }

    let ts = now_rfc3339();
    // A claim-only or profile-only name still gets a session row so the pause
    // flag has somewhere to live; registration preserves that flag.
    upsert_agent_session(db, user_id, &name, None, None, None, None, None, &ts).await?;
    let was_paused: Option<i64> =
        sqlx::query_scalar("SELECT paused FROM agent_sessions WHERE agent_name = ?1")
            .bind(&name)
            .fetch_optional(db)
            .await?;
    let changed = was_paused != Some(1);
    sqlx::query(
        "UPDATE agent_sessions
            SET paused = 1, paused_by = ?2, paused_at = ?3
          WHERE agent_name = ?1",
    )
    .bind(&name)
    .bind(user_id)
    .bind(&ts)
    .execute(db)
    .await?;
    sqlx::query("DELETE FROM chat_pushes WHERE member = ?1")
        .bind(format!("agent:{name}"))
        .execute(db)
        .await?;

    let entry = roster_entry(db, &name, limits)
        .await?
        .ok_or(AppError::NotFound)?;
    if changed {
        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &name,
                breadboard_core::AgentEventKind::Paused,
            )
            .summary("Paused; claims kept, new claims refused")
            .actor(Some(user_id)),
        )
        .await;
    }
    Ok((entry, changed))
}

/// Resume a paused name. Idempotent; returns whether the call changed
/// anything. Unknown names are a 404.
pub async fn resume_agent(
    db: &SqlitePool,
    raw_name: &str,
    limits: &ClaimLimits,
) -> AppResult<(AgentRosterEntry, bool)> {
    let name = names::normalize(raw_name);
    validate_agent_name(&name)?;
    let known = known_agent_names(db).await?;
    if !known.contains(&name) {
        return Err(AppError::NotFound);
    }

    let was_paused: Option<i64> =
        sqlx::query_scalar("SELECT paused FROM agent_sessions WHERE agent_name = ?1")
            .bind(&name)
            .fetch_optional(db)
            .await?;
    let changed = was_paused == Some(1);
    if changed {
        sqlx::query(
            "UPDATE agent_sessions
                SET paused = 0, paused_by = NULL, paused_at = NULL
              WHERE agent_name = ?1",
        )
        .bind(&name)
        .execute(db)
        .await?;
        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &name,
                breadboard_core::AgentEventKind::StateChange,
            )
            .summary("Resumed after a pause"),
        )
        .await;
    }
    let entry = roster_entry(db, &name, limits)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok((entry, changed))
}

/// Hide a known agent name from the default Agents view. Idempotent; claims
/// and chat identity are deliberately untouched.
pub async fn hide_agent(
    db: &SqlitePool,
    user_id: i64,
    raw_name: &str,
    limits: &ClaimLimits,
) -> AppResult<(AgentRosterEntry, bool)> {
    let name = names::normalize(raw_name);
    validate_agent_name(&name)?;
    let known = known_agent_names(db).await?;
    if !known.contains(&name) {
        return Err(AppError::NotFound);
    }
    let changed = sqlx::query(
        "INSERT INTO agent_hidden (agent_name, hidden_by, hidden_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(agent_name) DO NOTHING",
    )
    .bind(&name)
    .bind(user_id)
    .bind(now_rfc3339())
    .execute(db)
    .await?
    .rows_affected()
        > 0;
    let entry = roster_entry(db, &name, limits)
        .await?
        .ok_or(AppError::NotFound)?;
    if changed {
        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &name,
                breadboard_core::AgentEventKind::StateChange,
            )
            .summary("Hidden from the roster")
            .actor(Some(user_id)),
        )
        .await;
    }
    Ok((entry, changed))
}

/// Reveal a hidden name. Idempotent; returns whether a row was removed.
pub async fn unhide_agent(
    db: &SqlitePool,
    raw_name: &str,
    limits: &ClaimLimits,
) -> AppResult<(Option<AgentRosterEntry>, bool)> {
    let name = names::normalize(raw_name);
    validate_agent_name(&name)?;
    let changed = sqlx::query("DELETE FROM agent_hidden WHERE agent_name = ?1")
        .bind(&name)
        .execute(db)
        .await?
        .rows_affected()
        > 0;
    let entry = roster_entry(db, &name, limits).await?;
    if entry.is_none() && !changed {
        return Err(AppError::NotFound);
    }
    if changed {
        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &name,
                breadboard_core::AgentEventKind::StateChange,
            )
            .summary("Shown on the roster again"),
        )
        .await;
    }
    Ok((entry, changed))
}

/// Rename an agent. Claims, profile and session row move in one transaction;
/// a name owned by another user is refused rather than duplicated.
pub async fn rename_agent(
    db: &SqlitePool,
    user_id: i64,
    raw_old: &str,
    raw_new: &str,
    limits: &ClaimLimits,
) -> AppResult<AgentRosterEntry> {
    let old = names::normalize(raw_old);
    let new = names::normalize(raw_new);
    validate_agent_name(&old)?;
    validate_agent_name(&new)?;
    if old == new {
        return roster_entry(db, &old, limits)
            .await?
            .ok_or(AppError::NotFound);
    }

    let known = known_agent_names(db).await?;
    if !known.contains(&old) {
        return Err(AppError::NotFound);
    }
    if known.contains(&new) {
        return Err(AppError::Conflict(format!(
            "agent name '{new}' is already in use"
        )));
    }

    let foreign_claims: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_claims
          WHERE lower(agent_name) = ?1 AND claimed_by <> ?2",
    )
    .bind(&old)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    if foreign_claims > 0 {
        return Err(AppError::forbidden_msg(format!(
            "agent '{old}' holds claims owned by another user; cannot rename it"
        )));
    }
    let foreign_profiles: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_profiles WHERE agent_name = ?1 AND user_id <> ?2",
    )
    .bind(&old)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    if foreign_profiles > 0 {
        return Err(AppError::forbidden_msg(format!(
            "agent '{old}' has a profile owned by another user; cannot rename it"
        )));
    }
    let session_owner: Option<Option<i64>> =
        sqlx::query_scalar("SELECT user_id FROM agent_sessions WHERE agent_name = ?1")
            .bind(&old)
            .fetch_optional(db)
            .await?;
    if let Some(Some(owner)) = session_owner {
        if owner != user_id {
            return Err(AppError::forbidden_msg(format!(
                "agent '{old}' is registered by another user; cannot rename it"
            )));
        }
    }

    let mut tx = crate::db::begin_write(db).await?;
    sqlx::query("UPDATE agent_claims SET agent_name = ?2 WHERE lower(agent_name) = ?1")
        .bind(&old)
        .bind(&new)
        .execute(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
                AppError::Conflict("another claim already uses that agent name".into())
            }
            other => AppError::Db(other),
        })?;
    sqlx::query("UPDATE agent_profiles SET agent_name = ?2 WHERE agent_name = ?1")
        .bind(&old)
        .bind(&new)
        .execute(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
                AppError::Conflict("that agent name already has a profile".into())
            }
            other => AppError::Db(other),
        })?;
    sqlx::query("UPDATE agent_sessions SET agent_name = ?2 WHERE agent_name = ?1")
        .bind(&old)
        .bind(&new)
        .execute(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
                AppError::Conflict("that agent name is already registered".into())
            }
            other => AppError::Db(other),
        })?;
    // Keep the issue link's display name current; session ids stay the identity.
    sqlx::query("UPDATE issue_sessions SET agent_name = ?2 WHERE lower(agent_name) = ?1")
        .bind(&old)
        .bind(&new)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE agent_hidden SET agent_name = ?2 WHERE agent_name = ?1")
        .bind(&old)
        .bind(&new)
        .execute(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(dbe) if dbe.is_unique_violation() => {
                AppError::Conflict("that agent name is already hidden".into())
            }
            other => AppError::Db(other),
        })?;
    tx.commit().await?;

    roster_entry(db, &new, limits)
        .await?
        .ok_or(AppError::NotFound)
}

/// The union of sessions, profiles and claims, deduplicated by lowercased
/// agent name. This is what the Agents page renders.
pub async fn agent_roster(
    db: &SqlitePool,
    limits: &ClaimLimits,
) -> AppResult<Vec<AgentRosterEntry>> {
    let claims = list_claims(db).await?;
    let usage = claim_usage(db).await?;
    let profiles: Vec<AgentProfileRow> = sqlx::query_as(
        "SELECT lower(p.agent_name) AS agent_name, p.message, p.user_id,
                u.login AS login
           FROM agent_profiles p
           JOIN users u ON u.id = p.user_id
          ORDER BY p.updated_at ASC",
    )
    .fetch_all(db)
    .await?;
    let sessions: Vec<AgentSessionRow> =
        sqlx::query_as(&agent_session_join()).fetch_all(db).await?;
    let hidden: Vec<AgentHiddenRow> = sqlx::query_as(
        "SELECT h.agent_name, h.hidden_by, u.login AS hidden_by_login, h.hidden_at
           FROM agent_hidden h
           LEFT JOIN users u ON u.id = h.hidden_by",
    )
    .fetch_all(db)
    .await?;

    let mut map: HashMap<String, AgentRosterEntry> = HashMap::new();
    let mut claim_heartbeat: HashMap<String, DateTime<Utc>> = HashMap::new();
    let now = Utc::now();

    // Claims first: they give counts and, when there is no session row yet,
    // the only heartbeat/state the roster can show.
    for claim in &claims {
        let key = claim.agent_name.to_lowercase();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_roster_entry(&key));
        entry.claim_count += 1;
        match claim.mode {
            breadboard_core::ClaimMode::Exclusive => entry.exclusive_count += 1,
            breadboard_core::ClaimMode::Intent => entry.intent_count += 1,
        }
        if entry.user_id.is_none() {
            entry.user_id = Some(claim.claimed_by);
            entry.login = Some(claim.claimed_by_login.clone());
        }
        if entry.message.is_none() {
            entry.message = claim.message.clone();
        }
        let previous = claim_heartbeat.get(&key).copied();
        if previous.map(|at| claim.last_heartbeat > at).unwrap_or(true) {
            claim_heartbeat.insert(key.clone(), claim.last_heartbeat);
            entry.state = Some(claim.state);
            entry.note = claim.note.clone();
            if entry
                .last_seen
                .map(|at| claim.last_heartbeat > at)
                .unwrap_or(true)
            {
                entry.last_seen = Some(claim.last_heartbeat);
            }
        }
    }

    // Profiles can exist with no claim at all (the brioche-bridge case).
    for profile in &profiles {
        let key = profile.agent_name.to_lowercase();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_roster_entry(&key));
        entry.message = Some(profile.message.clone());
        if entry.user_id.is_none() {
            entry.user_id = Some(profile.user_id);
            entry.login = Some(profile.login.clone());
        }
        // A profile edit is not a session heartbeat, so it does not make the
        // row live; profile-only names stay stale until they register.
    }

    for session in &sessions {
        let key = session.agent_name.to_lowercase();
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| empty_roster_entry(&key));
        entry.session_id = session.session_id.clone();
        entry.parent_session_id = session.parent_session_id.clone();
        entry.host = session.host.clone();
        entry.pid = session.pid;
        if session.note.is_some() {
            entry.note = session.note.clone();
        }
        entry.first_seen = Some(parse_ts(&session.first_seen));
        entry.last_seen = Some(parse_ts(&session.last_seen));
        entry.paused = session.paused != 0;
        entry.paused_by = session.paused_by;
        entry.paused_by_login = session.paused_by_login.clone();
        entry.paused_at = session.paused_at.as_deref().map(parse_ts);
        if entry.user_id.is_none() {
            entry.user_id = session.user_id;
            entry.login = session.login.clone();
        }
    }

    // Hiding is display-only, so it is applied last and never changes counts.
    for hide in &hidden {
        let key = hide.agent_name.to_lowercase();
        if let Some(entry) = map.get_mut(&key) {
            entry.hidden = true;
            entry.hidden_by = hide.hidden_by;
            entry.hidden_by_login = hide.hidden_by_login.clone();
            entry.hidden_at = Some(parse_ts(&hide.hidden_at));
        }
    }

    let mut roster: Vec<AgentRosterEntry> = map
        .into_values()
        .map(|mut entry| {
            let live = !entry.paused
                && entry
                    .last_seen
                    .map(|at| (now - at).num_seconds() < AGENT_LIVE_SECS)
                    .unwrap_or(false);
            entry.live = live;
            entry.status = if entry.paused {
                "paused".to_string()
            } else if live {
                "live".to_string()
            } else {
                "stale".to_string()
            };
            // Budget visibility: per-agent usage is `exclusive_count`, and the
            // orchestrator roll-up counts the whole tree this name belongs to.
            entry.exclusive_limit = limits.per_agent;
            entry.orchestrator_exclusive_count = usage.tree_count(&entry.agent_name);
            entry.orchestrator_exclusive_limit = limits.per_orchestrator;
            entry
        })
        .collect();
    // Resolve parent session ids to orchestrator names after the union is
    // built. An unregistered parent keeps only the raw id, so the UI can show
    // it instead of dropping the link. This never participates in sorting.
    let by_session: HashMap<String, String> = roster
        .iter()
        .filter_map(|entry| {
            entry
                .session_id
                .as_ref()
                .map(|session| (session.clone(), entry.agent_name.clone()))
        })
        .collect();
    for entry in &mut roster {
        if let Some(parent) = entry.parent_session_id.clone() {
            if let Some(orchestrator) = by_session.get(&parent) {
                if orchestrator != &entry.agent_name {
                    entry.orchestrator = Some(orchestrator.clone());
                    entry.orchestrator_session_id = Some(parent);
                }
            }
        }
    }

    // Heartbeats flip live/stale constantly. Keep the server order stable by
    // sorting only on claim counts and name, with paused names last, so the
    // masonry layout does not reshuffle every 15 seconds.
    roster.sort_by(|a, b| {
        let rank = |e: &AgentRosterEntry| i64::from(e.paused);
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.exclusive_count.cmp(&a.exclusive_count))
            .then_with(|| a.agent_name.cmp(&b.agent_name))
    });
    Ok(roster)
}

pub async fn roster_entry(
    db: &SqlitePool,
    name: &str,
    limits: &ClaimLimits,
) -> AppResult<Option<AgentRosterEntry>> {
    let name = names::normalize(name);
    Ok(agent_roster(db, limits)
        .await?
        .into_iter()
        .find(|entry| entry.agent_name == name))
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

async fn hydrate_messages(
    db: &SqlitePool,
    rows: Vec<ChatMessageRow>,
) -> AppResult<Vec<ChatMessage>> {
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

pub async fn delete_chat_room(db: &SqlitePool, id: i64) -> AppResult<bool> {
    let changed = sqlx::query("DELETE FROM chat_rooms WHERE id = ?1")
        .bind(id)
        .execute(db)
        .await?
        .rows_affected();
    Ok(changed > 0)
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
        sqlx::query(
            "DELETE FROM chat_push_subs WHERE user_id = ?1 AND member = ?2 AND room_id = ?3",
        )
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
            UNION
            SELECT user_id FROM agent_sessions WHERE agent_name = ?1 AND user_id IS NOT NULL
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
        let subs: Vec<(i64, String)> =
            sqlx::query_as("SELECT user_id, member FROM chat_push_subs WHERE room_id = ?1")
                .bind(room.id)
                .fetch_all(db)
                .await?;
        for (user_id, member) in subs {
            if member != author_member {
                add_recipient(&mut want, member, user_id, ChatPushKind::Room);
            }
        }

        // @everyone is a room-wide ping: every known agent and every human,
        // not just the members who opted into room pushes.
        if mentions(&message.body)
            .iter()
            .any(|token| token == "everyone")
        {
            let agents: Vec<(i64, String)> = sqlx::query_as(
                "SELECT DISTINCT user_id, lower(agent_name) FROM (
                    SELECT user_id, agent_name FROM agent_profiles
                    UNION
                    SELECT claimed_by AS user_id, lower(agent_name) AS agent_name FROM agent_claims
                    UNION
                    SELECT user_id, agent_name FROM agent_sessions WHERE user_id IS NOT NULL
                 )",
            )
            .fetch_all(db)
            .await?;
            for (user_id, agent_name) in agents {
                let member = agent_member(&agent_name);
                if member != author_member {
                    add_recipient(&mut want, member, user_id, ChatPushKind::Ping);
                }
            }
            let humans: Vec<(i64, String)> = sqlx::query_as("SELECT id, login FROM users")
                .fetch_all(db)
                .await?;
            for (user_id, login) in humans {
                let member = human_member(&login);
                if member != author_member {
                    add_recipient(&mut want, member, user_id, ChatPushKind::Ping);
                }
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
                UNION
                SELECT user_id, agent_name FROM agent_sessions
                 WHERE user_id IS NOT NULL AND replace(lower(agent_name), ' ', '-') = ?1
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

    // Pause stops chat injection waking an agent: never enqueue a push for a
    // paused name, whether it came from a DM, a subscription, @everyone or a
    // direct mention.
    let paused: HashSet<String> =
        sqlx::query_scalar("SELECT agent_name FROM agent_sessions WHERE paused = 1")
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();

    for (member, (user_id, kind)) in want {
        if let Some(agent) = member.strip_prefix("agent:") {
            if paused.contains(agent) {
                continue;
            }
        }
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
            AND NOT EXISTS (
                SELECT 1 FROM agent_sessions s
                 WHERE s.paused = 1 AND p.member = 'agent:' || s.agent_name
            )
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
pub async fn find_or_create_dm(db: &SqlitePool, a: &str, b: &str) -> AppResult<ChatRoom> {
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
    if author_kind == ChatAuthorKind::Agent {
        if let Some(agent_name) = author_member.strip_prefix("agent:") {
            let summary = if body.trim().is_empty() {
                "Sent an attachment".to_string()
            } else {
                let mut text: String = body.trim().chars().take(80).collect();
                if body.trim().chars().count() > 80 {
                    text.push_str("...");
                }
                text
            };
            crate::agent_events::record_best_effort(
                db,
                crate::agent_events::AgentEventInsert::new(
                    agent_name,
                    breadboard_core::AgentEventKind::ChatSent,
                )
                .chat(room_id, Some(message.id))
                .summary(summary)
                .actor(Some(uploader_user_id)),
            )
            .await;
        }
    }
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
pub async fn delete_chat_message(db: &SqlitePool, id: i64, deleted_by: &str) -> AppResult<bool> {
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
    let row: Option<(i64, String, String, i64, String)> =
        sqlx::query_as("SELECT id, filename, mime, size, path FROM chat_uploads WHERE id = ?1")
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
           (SELECT COUNT(*) FROM agent_claims WHERE claimed_by = ?1 AND lower(agent_name) = ?2) +
           (SELECT COUNT(*) FROM agent_sessions WHERE user_id = ?1 AND agent_name = ?2)",
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
        tracing::info!("no projects yet: create one with POST /api/projects before filing issues");
    }
    Ok(())
}
