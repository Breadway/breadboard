//! BB-192: rename and repair move an agent's chat identity.
//!
//! Chat rows are keyed by the prefixed member string and a DM's unique key is
//! derived from the member pair. A rename that only moves claims and the
//! session row leaves the agent unable to see its DMs, so the move is part of
//! the rename transaction and repair covers identities already left behind.

mod common;
use common as tu;
use serde_json::{json, Value};

async fn register(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    name: &str,
    session: &str,
) -> reqwest::Response {
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&json!({ "agent_name": name, "session_id": session }))
        .send()
        .await
        .unwrap()
}

async fn open_dm(c: &reqwest::Client, base: &str, auth: &str, with: &str) -> Value {
    let response = c
        .post(format!("{base}/api/chat/dm"))
        .header("authorization", auth)
        .json(&json!({ "with": with }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    response.json().await.unwrap()
}

async fn post_message(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    room_id: i64,
    agent: Option<&str>,
    body: &str,
) -> Value {
    let mut payload = json!({ "room": room_id.to_string(), "body": body });
    if let Some(agent) = agent {
        payload["agent_name"] = json!(agent);
    }
    let response = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", auth)
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    response.json().await.unwrap()
}

async fn repair(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    old: &str,
    new: &str,
    dry_run: bool,
) -> reqwest::Response {
    c.post(format!("{base}/api/chat/identity/repair"))
        .header("authorization", auth)
        .json(&json!({ "old_name": old, "new_name": new, "dry_run": dry_run }))
        .send()
        .await
        .unwrap()
}

async fn count(db: &sqlx::SqlitePool, sql: &str, member: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(member)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn rename_moves_all_four_chat_tables_and_keeps_the_dm_single() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first = register(&c, &base, &auth, "old-chat", "ses_chat_rename").await;
    assert!(first.status().is_success(), "{}", first.status());
    let dm = open_dm(&c, &base, &auth, "old-chat").await;
    let room_id = dm["id"].as_i64().unwrap();
    assert_eq!(dm["members"], json!(["agent:old-chat", "human:tester"]));

    // A human message creates the agent's DM push. An agent message proves the
    // historical author stays untouched after the move.
    let human_message = post_message(&c, &base, &auth, room_id, None, "hello old-chat").await;
    let agent_message = post_message(
        &c,
        &base,
        &auth,
        room_id,
        Some("old-chat"),
        "hello from old",
    )
    .await;

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO chat_reads (room_id, member, last_read_message_id, updated_at)
         VALUES (?1, 'agent:old-chat', ?2, ?3)",
    )
    .bind(room_id)
    .bind(human_message["id"].as_i64().unwrap())
    .bind(&now)
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_push_subs (user_id, member, room_id, created_at)
         VALUES (1, 'agent:old-chat', 1, ?1)",
    )
    .bind(&now)
    .execute(&app.db)
    .await
    .unwrap();

    let renamed = c
        .post(format!("{base}/api/agents/old-chat/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "new-chat" }))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);

    for (table, column) in [
        ("chat_room_members", "member"),
        ("chat_reads", "member"),
        ("chat_pushes", "member"),
        ("chat_push_subs", "member"),
    ] {
        let old_count = count(
            &app.db,
            &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
            "agent:old-chat",
        )
        .await;
        assert_eq!(old_count, 0, "{table}: old member survived");
        let new_count = count(
            &app.db,
            &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
            "agent:new-chat",
        )
        .await;
        assert_eq!(new_count, 1, "{table}: new member missing");
    }

    let new_key: String = sqlx::query_scalar("SELECT dm_key FROM chat_rooms WHERE id = ?1")
        .bind(room_id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(new_key, "agent:new-chat|human:tester");

    // History is not rewritten: the old author keeps both member and display.
    let author: (String, String) =
        sqlx::query_as("SELECT author_member, author_display FROM chat_messages WHERE id = ?1")
            .bind(agent_message["id"].as_i64().unwrap())
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(author.0, "agent:old-chat");
    assert_eq!(author.1, "old-chat");

    // The new name sees the DM, and opening it returns the same room instead
    // of creating a duplicate under the new key.
    let rooms: Value = c
        .get(format!("{base}/api/chat/rooms?agent=new-chat"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        rooms
            .as_array()
            .unwrap()
            .iter()
            .any(|room| room["id"] == room_id),
        "{rooms}"
    );
    let reopened = open_dm(&c, &base, &auth, "new-chat").await;
    assert_eq!(reopened["id"], room_id);
    let dm_rooms: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_rooms WHERE kind = 'dm'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(dm_rooms, 1, "rename created a duplicate DM room");
}

#[tokio::test]
async fn repair_moves_a_stale_names_chat_identity_with_a_dry_run_count() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let old = register(&c, &base, &auth, "stale-chat", "ses_chat_stale").await;
    assert!(old.status().is_success(), "{}", old.status());
    let new = register(&c, &base, &auth, "fresh-chat", "ses_chat_fresh").await;
    assert!(new.status().is_success(), "{}", new.status());

    let dm = open_dm(&c, &base, &auth, "stale-chat").await;
    let room_id = dm["id"].as_i64().unwrap();
    let message = post_message(&c, &base, &auth, room_id, None, "for stale-chat").await;
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO chat_reads (room_id, member, last_read_message_id, updated_at)
         VALUES (?1, 'agent:stale-chat', ?2, ?3)",
    )
    .bind(room_id)
    .bind(message["id"].as_i64().unwrap())
    .bind(&now)
    .execute(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_push_subs (user_id, member, room_id, created_at)
         VALUES (1, 'agent:stale-chat', 1, ?1)",
    )
    .bind(&now)
    .execute(&app.db)
    .await
    .unwrap();

    // A stale session row does not block the repair; only a live one does.
    sqlx::query("UPDATE agent_sessions SET last_seen = ?1 WHERE agent_name = 'stale-chat'")
        .bind((chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    let dry = repair(&c, &base, &auth, "stale-chat", "fresh-chat", true).await;
    assert_eq!(dry.status(), 200);
    let dry: Value = dry.json().await.unwrap();
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["counts"]["members"], 1);
    assert_eq!(dry["counts"]["reads"], 1);
    assert_eq!(dry["counts"]["pushes"], 1);
    assert_eq!(dry["counts"]["push_subs"], 1);
    assert_eq!(dry["counts"]["dm_rooms"], 1);
    let still_old: i64 = count(
        &app.db,
        "SELECT COUNT(*) FROM chat_room_members WHERE member = ?1",
        "agent:stale-chat",
    )
    .await;
    assert_eq!(still_old, 1, "dry run wrote chat rows");

    let moved = repair(&c, &base, &auth, "stale-chat", "fresh-chat", false).await;
    assert_eq!(moved.status(), 200);
    let moved: Value = moved.json().await.unwrap();
    assert_eq!(moved["dry_run"], false);
    assert_eq!(moved["counts"]["members"], 1);

    for table in [
        "chat_room_members",
        "chat_reads",
        "chat_pushes",
        "chat_push_subs",
    ] {
        let old_count = count(
            &app.db,
            &format!("SELECT COUNT(*) FROM {table} WHERE member = ?1"),
            "agent:stale-chat",
        )
        .await;
        assert_eq!(old_count, 0, "{table}: old member survived the repair");
        let new_count = count(
            &app.db,
            &format!("SELECT COUNT(*) FROM {table} WHERE member = ?1"),
            "agent:fresh-chat",
        )
        .await;
        assert_eq!(new_count, 1, "{table}: new member missing after repair");
    }

    let reopened = open_dm(&c, &base, &auth, "fresh-chat").await;
    assert_eq!(reopened["id"], room_id);
    assert_eq!(
        reopened["members"],
        json!(["agent:fresh-chat", "human:tester"])
    );
}

#[tokio::test]
async fn repair_refuses_while_the_old_name_has_a_live_session() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let live = register(&c, &base, &auth, "busy-chat", "ses_chat_live").await;
    assert!(live.status().is_success(), "{}", live.status());
    let dm = open_dm(&c, &base, &auth, "busy-chat").await;
    let room_id = dm["id"].as_i64().unwrap();

    let refused = repair(&c, &base, &auth, "busy-chat", "quiet-chat", false).await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("live"), "{body}");

    let still_old: i64 = count(
        &app.db,
        "SELECT COUNT(*) FROM chat_room_members WHERE member = ?1",
        "agent:busy-chat",
    )
    .await;
    assert_eq!(still_old, 1);
    let dm_key: String = sqlx::query_scalar("SELECT dm_key FROM chat_rooms WHERE id = ?1")
        .bind(room_id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(dm_key, "agent:busy-chat|human:tester");
}

#[tokio::test]
async fn repair_merges_into_an_existing_dm_for_the_new_name() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let old = register(&c, &base, &auth, "left-chat", "ses_chat_left").await;
    assert!(old.status().is_success(), "{}", old.status());
    let new = register(&c, &base, &auth, "right-chat", "ses_chat_right").await;
    assert!(new.status().is_success(), "{}", new.status());
    sqlx::query("UPDATE agent_sessions SET last_seen = ?1 WHERE agent_name = 'left-chat'")
        .bind((chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    // Production shape: a DM already exists under both names.
    let old_dm = open_dm(&c, &base, &auth, "left-chat").await;
    let old_room = old_dm["id"].as_i64().unwrap();
    let new_dm = open_dm(&c, &base, &auth, "right-chat").await;
    let new_room = new_dm["id"].as_i64().unwrap();
    assert_ne!(old_room, new_room);
    let message = post_message(
        &c,
        &base,
        &auth,
        old_room,
        None,
        "history from the old name",
    )
    .await;
    let message_id = message["id"].as_i64().unwrap();

    let moved = repair(&c, &base, &auth, "left-chat", "right-chat", false).await;
    assert_eq!(moved.status(), 200);
    let moved: Value = moved.json().await.unwrap();
    assert_eq!(moved["counts"]["dm_rooms"], 1);

    let dm_rooms: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_rooms WHERE kind = 'dm'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(dm_rooms, 1, "the old DM was not merged into the new one");
    let gone: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_rooms WHERE id = ?1")
        .bind(old_room)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(gone, 0, "old DM room survived the merge");
    let message_room: i64 = sqlx::query_scalar("SELECT room_id FROM chat_messages WHERE id = ?1")
        .bind(message_id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(message_room, new_room, "history did not follow the merge");
    let reopened = open_dm(&c, &base, &auth, "right-chat").await;
    assert_eq!(reopened["id"], new_room);
}
