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
