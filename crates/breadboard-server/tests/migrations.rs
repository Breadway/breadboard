//! Migration 0020: pre-existing killed tombstones are removed, not kept.

use sqlx::SqlitePool;

const MIGRATIONS: &[&str] = &[
    include_str!("../../../migrations/0001_init.sql"),
    include_str!("../../../migrations/0002_sessions.sql"),
    include_str!("../../../migrations/0003_device_codes.sql"),
    include_str!("../../../migrations/0004_issue_milestone.sql"),
    include_str!("../../../migrations/0005_notification_prefs.sql"),
    include_str!("../../../migrations/0006_digest_tracking.sql"),
    include_str!("../../../migrations/0007_cycle_projects.sql"),
    include_str!("../../../migrations/0008_agent_claims.sql"),
    include_str!("../../../migrations/0009_claim_intents.sql"),
    include_str!("../../../migrations/0010_agent_profiles.sql"),
    include_str!("../../../migrations/0011_chat.sql"),
    include_str!("../../../migrations/0012_chat_uploads.sql"),
    include_str!("../../../migrations/0013_chat_reads.sql"),
    include_str!("../../../migrations/0014_chat_push.sql"),
    include_str!("../../../migrations/0015_chat_message_state.sql"),
    include_str!("../../../migrations/0016_issue_list_indexes.sql"),
    include_str!("../../../migrations/0017_agent_sessions.sql"),
    include_str!("../../../migrations/0018_agent_hidden.sql"),
    include_str!("../../../migrations/0019_agent_parent_session.sql"),
];

/// Read migrations from the repository in filename order, stopping before the
/// named file so a migration can be tested against realistic pre-state.
fn migrations_before(stop: &str) -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".sql") && name.as_str() < stop)
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|name| {
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../migrations")
                    .join(name),
            )
            .unwrap()
        })
        .collect()
}

/// BB-170: existing duplicate sessions on one name resolve to the most
/// recently active session. Claims from the displaced session stay in place
/// and the displaced id is recorded so it is discoverable, not silently lost.
#[tokio::test]
async fn migration_0028_resolves_duplicate_live_names_without_losing_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}?mode=rwc",
        tmp.path().join("duplicate-names.db").display()
    ))
    .await
    .unwrap();
    for sql in migrations_before("0028") {
        sqlx::raw_sql(&sql).execute(&pool).await.unwrap();
    }

    sqlx::query(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (1, 'tester', 'Tester')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO projects (id, key, name, next_number) VALUES (1, 'AGENT', 'Agent work', 3)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (id, key) in [(1_i64, "AGENT-1"), (2_i64, "AGENT-2")] {
        sqlx::query(
            "INSERT INTO issues
                (id, key, project_id, title, created_at, updated_at)
             VALUES (?1, ?2, 1, 'duplicate name fixture',
                     '2026-11-09T08:00:00Z', '2026-11-09T08:00:00Z')",
        )
        .bind(id)
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    }

    // The production shape: one name row already overwritten by the newest
    // registration, while the older session's claim is still attributed to it.
    sqlx::query(
        "INSERT INTO agent_sessions
            (agent_name, session_id, user_id, first_seen, last_seen)
         VALUES ('duplicate', 'ses_old', 1, '2026-11-09T08:00:00Z', '2026-11-09T08:10:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();

    for (issue_id, session_id, heartbeat) in [
        (1_i64, "ses_old", "2026-11-09T08:10:00Z"),
        (2_i64, "ses_new", "2026-11-09T09:30:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO agent_claims
                (issue_id, claimed_by, agent_name, mode, session_id, state,
                 board_status, claimed_at, updated_at, last_heartbeat)
             VALUES (?1, 1, 'duplicate', 'exclusive', ?2, 'implementing',
                     'doing', '2026-11-09T08:00:00Z', ?3, ?3)",
        )
        .bind(issue_id)
        .bind(session_id)
        .bind(heartbeat)
        .execute(&pool)
        .await
        .unwrap();
    }

    sqlx::raw_sql(include_str!(
        "../../../migrations/0028_unique_live_agent_name.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let holder: (String, String) = sqlx::query_as(
        "SELECT session_id, last_seen FROM agent_sessions WHERE agent_name = 'duplicate'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(holder.0, "ses_new", "newest session must hold the name");
    assert_eq!(holder.1, "2026-11-09T09:30:00Z");

    let claims: Vec<(i64, String)> = sqlx::query_as(
        "SELECT issue_id, session_id FROM agent_claims
          WHERE agent_name = 'duplicate' ORDER BY issue_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        claims,
        vec![(1, "ses_old".to_string()), (2, "ses_new".to_string())],
        "no claim may be dropped or reattributed"
    );

    let conflicts: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT agent_name, session_id, holder_session_id
           FROM agent_name_conflicts",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        conflicts,
        vec![(
            "duplicate".to_string(),
            "ses_old".to_string(),
            "ses_new".to_string()
        )],
        "the displaced session must stay discoverable"
    );
}

#[tokio::test]
async fn migration_0029_keeps_one_name_per_session_and_preserves_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}?mode=rwc",
        tmp.path().join("one-name-per-session.db").display()
    ))
    .await
    .unwrap();
    for sql in migrations_before("0029") {
        sqlx::raw_sql(&sql).execute(&pool).await.unwrap();
    }

    sqlx::query(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (1, 'tester', 'Tester')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO projects (id, key, name, next_number) VALUES (1, 'AGENT', 'Agent work', 4)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (id, key) in [(1_i64, "AGENT-1"), (2_i64, "AGENT-2"), (3_i64, "AGENT-3")] {
        sqlx::query(
            "INSERT INTO issues
                (id, key, project_id, title, created_at, updated_at)
             VALUES (?1, ?2, 1, 'multi-name fixture',
                     '2026-11-10T05:00:00Z', '2026-11-10T05:00:00Z')",
        )
        .bind(id)
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Two sessions with accumulated names. "ses_multi" has a claim under the
    // older name; "ses_claims" has a claim under each name, so its extra must
    // stay registered even though it loses the ranking.
    for (name, session_id, last_seen) in [
        ("study pwa", "ses_multi", "2026-11-10T05:00:00Z"),
        ("doughcartographer", "ses_multi", "2026-11-10T06:30:00Z"),
        ("butter crumb", "ses_claims", "2026-11-10T07:00:00Z"),
        ("doughy dynamo", "ses_claims", "2026-11-10T08:00:00Z"),
        ("keeper", "ses_single", "2026-11-10T09:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO agent_sessions
                (agent_name, session_id, user_id, first_seen, last_seen)
             VALUES (?1, ?2, 1, '2026-11-10T05:00:00Z', ?3)",
        )
        .bind(name)
        .bind(session_id)
        .bind(last_seen)
        .execute(&pool)
        .await
        .unwrap();
    }

    for (issue_id, name, session_id, heartbeat) in [
        (1_i64, "study pwa", "ses_multi", "2026-11-10T05:30:00Z"),
        (2_i64, "butter crumb", "ses_claims", "2026-11-10T07:30:00Z"),
        (3_i64, "doughy dynamo", "ses_claims", "2026-11-10T08:30:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO agent_claims
                (issue_id, claimed_by, agent_name, mode, session_id, state,
                 board_status, claimed_at, updated_at, last_heartbeat)
             VALUES (?1, 1, ?2, 'exclusive', ?3, 'implementing',
                     'doing', '2026-11-10T05:00:00Z', ?4, ?4)",
        )
        .bind(issue_id)
        .bind(name)
        .bind(session_id)
        .bind(heartbeat)
        .execute(&pool)
        .await
        .unwrap();
    }

    sqlx::raw_sql(include_str!(
        "../../../migrations/0029_one_name_per_session.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let multi: Vec<String> = sqlx::query_scalar(
        "SELECT agent_name FROM agent_sessions WHERE session_id = 'ses_multi' ORDER BY agent_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        multi,
        vec!["study pwa".to_string()],
        "the claimed name must win over the newer claimless name"
    );
    let dropped: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_sessions WHERE agent_name = 'doughcartographer'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dropped, 0, "the claimless extra row must be deleted");

    let claim_backed: Vec<String> = sqlx::query_scalar(
        "SELECT agent_name FROM agent_sessions WHERE session_id = 'ses_claims' ORDER BY agent_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        claim_backed,
        vec!["butter crumb".to_string(), "doughy dynamo".to_string()],
        "an extra name that still holds a claim must not be orphaned"
    );

    let keeper: String =
        sqlx::query_scalar("SELECT agent_name FROM agent_sessions WHERE session_id = 'ses_single'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(keeper, "keeper");

    let audit: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT session_id, dropped_name, kept_name, claims_on_dropped, dropped
           FROM agent_session_conflicts
          ORDER BY session_id, dropped_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit,
        vec![
            (
                "ses_claims".to_string(),
                "butter crumb".to_string(),
                "doughy dynamo".to_string(),
                1,
                0
            ),
            (
                "ses_multi".to_string(),
                "doughcartographer".to_string(),
                "study pwa".to_string(),
                0,
                1
            ),
        ],
        "every resolved extra must be recorded with whether it was dropped"
    );

    let claims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_claims")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(claims, 3, "no claim may be dropped or reattributed");
}

#[tokio::test]
async fn migration_removes_pre_existing_killed_rows_and_keeps_audit() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = SqlitePool::connect(&format!(
        "sqlite://{}?mode=rwc",
        tmp.path().join("migrations.db").display()
    ))
    .await
    .unwrap();
    for sql in MIGRATIONS {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }

    sqlx::query(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (1, 'tester', 'Tester')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO chat_messages
            (room_id, author_member, author_display, author_kind, body, created_at)
         VALUES (1, 'human:tester', 'tester', 'human', 'hello ghosts', '2026-11-08T10:02:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Two legacy BB-143/BB-146 killed tombstones, each with a profile, hidden
    // flag, push, push subscription and read cursor. A scalar subquery would
    // only clean the first name, so this fixture guards the row-set form.
    for (name, session_id, killed_at) in [
        ("ghost-one", "ses_ghost_one", "2026-11-08T10:05:00Z"),
        ("ghost-two", "ses_ghost_two", "2026-11-08T10:06:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO agent_sessions
                (agent_name, session_id, user_id, first_seen, last_seen, killed, killed_by, killed_at)
             VALUES (?1, ?2, 1, '2026-11-08T10:00:00Z', '2026-11-08T10:00:00Z', 1, 1, ?3)",
        )
        .bind(name)
        .bind(session_id)
        .bind(killed_at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_profiles (user_id, agent_name, message, updated_at)
             VALUES (1, ?1, 'killed but still here', '2026-11-08T10:00:00Z')",
        )
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_hidden (agent_name, hidden_by, hidden_at)
             VALUES (?1, 1, '2026-11-08T10:01:00Z')",
        )
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
        let member = format!("agent:{name}");
        sqlx::query(
            "INSERT INTO chat_pushes (user_id, member, room_id, message_id, kind, created_at)
             VALUES (1, ?1, 1, 1, 'dm', '2026-11-08T10:02:00Z')",
        )
        .bind(&member)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_push_subs (user_id, member, room_id, created_at)
             VALUES (1, ?1, 1, '2026-11-08T10:02:00Z')",
        )
        .bind(&member)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_reads (room_id, member, last_read_message_id, updated_at)
             VALUES (1, ?1, 1, '2026-11-08T10:02:00Z')",
        )
        .bind(&member)
        .execute(&pool)
        .await
        .unwrap();
    }

    // A live agent must survive the cleanup.
    sqlx::query(
        "INSERT INTO agent_sessions
            (agent_name, session_id, user_id, first_seen, last_seen, killed)
         VALUES ('keeper', 'ses_keeper', 1, '2026-11-08T10:00:00Z', '2026-11-08T10:00:00Z', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(include_str!(
        "../../../migrations/0020_agent_removal_and_pause.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    for (name, killed_at) in [
        ("ghost-one", "2026-11-08T10:05:00Z"),
        ("ghost-two", "2026-11-08T10:06:00Z"),
    ] {
        let sessions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_sessions WHERE agent_name = ?1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sessions, 0, "{name}: killed session row survived");
        let profiles: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_profiles WHERE agent_name = ?1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(profiles, 0, "{name}: killed profile survived");
        let hidden: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_hidden WHERE agent_name = ?1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(hidden, 0, "{name}: killed hidden flag survived");
        let member = format!("agent:{name}");
        let pushes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_pushes WHERE member = ?1")
            .bind(&member)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pushes, 0, "{name}: killed chat push survived");
        let subs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_push_subs WHERE member = ?1")
            .bind(&member)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(subs, 0, "{name}: killed push subscription survived");
        let reads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_reads WHERE member = ?1")
            .bind(&member)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(reads, 0, "{name}: killed read cursor survived");

        let audit: (String, i64, String) = sqlx::query_as(
            "SELECT reason, removed_by, removed_at FROM agent_removals WHERE agent_name = ?1",
        )
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(audit.0.contains("pre-existing killed row"));
        assert_eq!(audit.1, 1, "{name}: killer");
        assert_eq!(audit.2, killed_at, "{name}: removal time");
    }

    let keeper: (i64, i64) =
        sqlx::query_as("SELECT paused, killed FROM agent_sessions WHERE agent_name = 'keeper'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(keeper, (0, 0));
}
