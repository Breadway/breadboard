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

    // A legacy BB-143/BB-146 killed tombstone with a profile and hidden flag.
    sqlx::query(
        "INSERT INTO agent_sessions
            (agent_name, session_id, user_id, first_seen, last_seen, killed, killed_by, killed_at)
         VALUES ('ghost', 'ses_ghost', 1, '2026-11-08T10:00:00Z', '2026-11-08T10:00:00Z', 1, 1,
                 '2026-11-08T10:05:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_profiles (user_id, agent_name, message, updated_at)
         VALUES (1, 'ghost', 'killed but still here', '2026-11-08T10:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_hidden (agent_name, hidden_by, hidden_at)
         VALUES ('ghost', 1, '2026-11-08T10:01:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_messages
            (room_id, author_member, author_display, author_kind, body, created_at)
         VALUES (1, 'human:tester', 'tester', 'human', 'hello ghost', '2026-11-08T10:02:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_pushes (user_id, member, room_id, message_id, kind, created_at)
         VALUES (1, 'agent:ghost', 1, 1, 'dm', '2026-11-08T10:02:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_reads (room_id, member, last_read_message_id, updated_at)
         VALUES (1, 'agent:ghost', 1, '2026-11-08T10:02:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();

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

    let sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_sessions WHERE agent_name = 'ghost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sessions, 0, "killed session row survived");
    let profiles: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_profiles WHERE agent_name = 'ghost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(profiles, 0, "killed profile survived");
    let hidden: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_hidden WHERE agent_name = 'ghost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(hidden, 0, "killed hidden flag survived");
    let pushes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_pushes WHERE member = 'agent:ghost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pushes, 0, "killed chat push survived");
    let reads: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_reads WHERE member = 'agent:ghost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reads, 0, "killed read cursor survived");

    let audit: (String, i64, String) = sqlx::query_as(
        "SELECT reason, removed_by, removed_at FROM agent_removals WHERE agent_name = 'ghost'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audit.0.contains("pre-existing killed row"));
    assert_eq!(audit.1, 1);
    assert_eq!(audit.2, "2026-11-08T10:05:00Z");

    let keeper: (i64, i64) =
        sqlx::query_as("SELECT paused, killed FROM agent_sessions WHERE agent_name = 'keeper'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(keeper, (0, 0));
}
