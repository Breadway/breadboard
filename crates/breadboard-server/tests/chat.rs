//! Chat: shared rooms, custom rooms, DMs, and agent/human authoring.

mod common;
use common as tu;
use serde_json::{json, Value};

async fn setup(c: &reqwest::Client, base: &str, auth: &str) {
    c.post(format!("{base}/api/projects"))
        .header("authorization", auth)
        .json(&json!({ "key": "CHT", "name": "Chat" }))
        .send()
        .await
        .unwrap();
}

/// Claiming an issue is what makes an agent name real for chat auth.
async fn make_agent(c: &reqwest::Client, base: &str, auth: &str, agent: &str) {
    let issue: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": format!("work for {agent}"), "project": "CHT" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = issue["key"].as_str().unwrap();
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", auth)
        .json(&json!({ "agent_name": agent }))
        .send()
        .await
        .unwrap();
}

async fn rooms(c: &reqwest::Client, base: &str, auth: &str, agent: Option<&str>) -> Value {
    let url = match agent {
        Some(a) => format!("{base}/api/chat/rooms?agent={a}"),
        None => format!("{base}/api/chat/rooms"),
    };
    c.get(url)
        .header("authorization", auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn lobby_accepts_humans_and_agents() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;
    make_agent(&c, &base, &auth, "Crumb Baron").await;

    let human: Value = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "body": "hello agents" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(human["author_kind"], "human");
    assert_eq!(human["author_display"], "tester");

    let agent: Value = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Crumb Baron", "body": "hello humans" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(agent["author_kind"], "agent");
    assert_eq!(agent["author_display"], "crumb baron");

    let list = rooms(&c, &base, &auth, None).await;
    let lobby = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "lobby")
        .unwrap();
    assert_eq!(lobby["last_message"]["body"], "hello humans");

    let msgs: Value = c
        .get(format!(
            "{base}/api/chat/rooms/{}/messages",
            lobby["id"].as_i64().unwrap()
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(msgs.as_array().unwrap().len(), 2);

    // An agent that does not belong to this user cannot speak.
    let forged = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "not mine", "body": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 403);
}

#[tokio::test]
async fn custom_rooms_are_creatable_and_unique() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;

    let created: Value = c
        .post(format!("{base}/api/chat/rooms"))
        .header("authorization", &auth)
        .json(&json!({ "name": "Bread Puns!", "description": "knead to know" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["name"], "bread-puns", "names are slugified");
    assert_eq!(created["description"], "knead to know");

    let dup = c
        .post(format!("{base}/api/chat/rooms"))
        .header("authorization", &auth)
        .json(&json!({ "name": "bread-puns" }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    let list = rooms(&c, &base, &auth, None).await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "bread-puns"));
}

#[tokio::test]
async fn dms_are_private_to_their_agents_but_visible_to_humans() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;
    for agent in ["Alpha", "Beta", "Gamma"] {
        make_agent(&c, &base, &auth, agent).await;
    }

    // Alpha -> Beta DM, and Gamma -> Beta DM.
    let ab: Value = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "to": "Beta", "body": "a to b" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Gamma", "to": "Beta", "body": "g to b" }))
        .send()
        .await
        .unwrap();

    let alpha_rooms = rooms(&c, &base, &auth, Some("Alpha")).await;
    let alpha_dms: Vec<&Value> = alpha_rooms
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "dm")
        .collect();
    assert_eq!(alpha_dms.len(), 1, "Alpha sees only its own DM");
    assert_eq!(alpha_dms[0]["id"], ab["room_id"]);

    // A human sees every DM.
    let all = rooms(&c, &base, &auth, None).await;
    let dm_count = all
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "dm")
        .count();
    assert_eq!(dm_count, 2);

    // Gamma cannot read Alpha/Beta's messages even by room id.
    let forbidden = c
        .get(format!(
            "{base}/api/chat/rooms/{}/messages?agent=Gamma",
            ab["room_id"].as_i64().unwrap()
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
}

#[tokio::test]
async fn chat_rejects_empty_and_huge_messages() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;

    let empty = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "body": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), 400);

    let huge = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "body": "x".repeat(2001) }))
        .send()
        .await
        .unwrap();
    assert_eq!(huge.status(), 400);
}

/// Uploads attach to a message once, render inline for safe images, and reuse
/// is rejected.
#[tokio::test]
async fn uploads_attach_to_messages() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;

    // 1x1 transparent PNG.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let upload: Value = c
        .post(format!("{base}/api/chat/uploads/base64"))
        .header("authorization", &auth)
        .json(&json!({ "filename": "dough.png", "mime": "image/png", "data_base64": png }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let upload_id = upload["id"].as_i64().unwrap();
    assert_eq!(upload["filename"], "dough.png");
    assert_eq!(upload["mime"], "image/png");

    let message: Value = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "body": "look at this", "uploads": [upload_id] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(message["attachments"][0]["filename"], "dough.png");

    let downloaded = c
        .get(format!("{base}/api/chat/uploads/{upload_id}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(downloaded.status(), 200);
    assert_eq!(downloaded.headers()["content-type"], "image/png");
    assert_eq!(downloaded.headers()["x-content-type-options"], "nosniff");

    // A second message cannot reuse the same upload.
    let reuse = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "body": "again", "uploads": [upload_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(reuse.status(), 400);
}

/// Read cursors: unread counts drop when a room is read, and your own
/// messages never count as unread.
#[tokio::test]
async fn unread_counts_and_cursors() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;
    for agent in ["Alpha", "Beta"] {
        make_agent(&c, &base, &auth, agent).await;
    }

    // Beta says two things; Alpha has never read the lobby.
    for text in ["first", "second"] {
        c.post(format!("{base}/api/chat/messages"))
            .header("authorization", &auth)
            .json(&json!({ "agent_name": "Beta", "body": text }))
            .send()
            .await
            .unwrap();
    }

    let alpha_rooms = rooms(&c, &base, &auth, Some("Alpha")).await;
    let lobby = alpha_rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "lobby")
        .unwrap();
    assert_eq!(lobby["unread"], 2, "Beta's two messages are unread");

    // Alpha reads them.
    c.get(format!(
        "{base}/api/chat/rooms/{}/messages?agent=Alpha",
        lobby["id"].as_i64().unwrap()
    ))
    .header("authorization", &auth)
    .send()
    .await
    .unwrap();

    let alpha_rooms = rooms(&c, &base, &auth, Some("Alpha")).await;
    let lobby = alpha_rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "lobby")
        .unwrap();
    assert_eq!(lobby["unread"], 0);

    // Alpha replies: Beta now has two unread (Beta never read), and Alpha's
    // own message does not re-add unread for Alpha.
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "body": "third" }))
        .send()
        .await
        .unwrap();
    let alpha_rooms = rooms(&c, &base, &auth, Some("Alpha")).await;
    let lobby = alpha_rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "lobby")
        .unwrap();
    assert_eq!(lobby["unread"], 0, "own messages are not unread");

    let beta_rooms = rooms(&c, &base, &auth, Some("Beta")).await;
    let lobby = beta_rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "lobby")
        .unwrap();
    assert_eq!(
        lobby["unread"], 1,
        "Beta's own messages are excluded; only Alpha's reply is unread"
    );

    // Humans get unread too.
    let human_rooms = rooms(&c, &base, &auth, None).await;
    let lobby = human_rooms
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "lobby")
        .unwrap();
    assert_eq!(lobby["unread"], 3);
}

async fn pushes(c: &reqwest::Client, base: &str, auth: &str, agent: Option<&str>) -> Value {
    let url = match agent {
        Some(a) => format!("{base}/api/chat/pushes?agent={a}"),
        None => format!("{base}/api/chat/pushes"),
    };
    c.get(url)
        .header("authorization", auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn dms_always_push_and_pings_bypass_subscriptions() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;
    for agent in ["Alpha", "Beta", "Crumb Baron"] {
        make_agent(&c, &base, &auth, agent).await;
    }

    // DM: always pushes to the other side.
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "to": "Beta", "body": "private note" }))
        .send()
        .await
        .unwrap();
    let beta = pushes(&c, &base, &auth, Some("Beta")).await;
    assert_eq!(beta.as_array().unwrap().len(), 1);
    assert_eq!(beta[0]["kind"], "dm");
    let alpha = pushes(&c, &base, &auth, Some("Alpha")).await;
    assert_eq!(alpha.as_array().unwrap().len(), 0, "author is not pushed");

    // Group room: no push without a subscription.
    let room: Value = c
        .post(format!("{base}/api/chat/rooms"))
        .header("authorization", &auth)
        .json(&json!({ "name": "puns" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let room_id = room["id"].as_i64().unwrap();
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "room": room_id.to_string(), "body": "no ping here" }))
        .send()
        .await
        .unwrap();
    let beta = pushes(&c, &base, &auth, Some("Beta")).await;
    assert_eq!(beta.as_array().unwrap().len(), 1, "no room subscription");

    // Subscribe Beta, then a room message pushes.
    let sub = c
        .put(format!("{base}/api/chat/push-subs"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Beta", "room": room_id.to_string(), "enabled": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(sub.status(), 204);
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "room": room_id.to_string(), "body": "room news" }))
        .send()
        .await
        .unwrap();
    let beta = pushes(&c, &base, &auth, Some("Beta")).await;
    assert_eq!(beta.as_array().unwrap().len(), 2);
    assert_eq!(beta[1]["kind"], "room");

    // Ping works without a subscription, including a spaced agent name.
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "room": room_id.to_string(), "body": "hey @crumb-baron look" }))
        .send()
        .await
        .unwrap();
    let baron = pushes(&c, &base, &auth, Some("Crumb Baron")).await;
    assert_eq!(baron[0]["kind"], "ping");
    assert_eq!(baron[0]["message"]["body"], "hey @crumb-baron look");

    // Humans can ping too (by login).
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "body": "ping @tester" }))
        .send()
        .await
        .unwrap();
    let human = pushes(&c, &base, &auth, None).await;
    assert_eq!(human.as_array().unwrap().len(), 1);
    assert_eq!(human[0]["kind"], "ping");
}

#[tokio::test]
async fn pushes_acknowledge() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    setup(&c, &base, &auth).await;
    for agent in ["Alpha", "Beta"] {
        make_agent(&c, &base, &auth, agent).await;
    }

    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Alpha", "to": "Beta", "body": "yo" }))
        .send()
        .await
        .unwrap();
    let beta = pushes(&c, &base, &auth, Some("Beta")).await;
    let up_to = beta[0]["id"].as_i64().unwrap();

    let ack = c
        .post(format!("{base}/api/chat/pushes/ack"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Beta", "up_to": up_to }))
        .send()
        .await
        .unwrap();
    assert_eq!(ack.status(), 204);

    let beta = pushes(&c, &base, &auth, Some("Beta")).await;
    assert_eq!(beta.as_array().unwrap().len(), 0);
}
