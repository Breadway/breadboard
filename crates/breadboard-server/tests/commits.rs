//! Commit attribution: record which agent, session, orchestrator and model
//! produced a commit, resolve a rewritten commit, and roll an issue's commits
//! up for the issue and its linked PR card.

mod common;
use common as tu;
use serde_json::{json, Value};

async fn project_and_issue(c: &reqwest::Client, base: &str, auth: &str) -> String {
    let resp = c
        .post(format!("{base}/api/projects"))
        .header("authorization", auth)
        .json(&json!({ "key": "COMMIT", "name": "Commit attribution" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "project setup failed");
    let issue: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": "attribution target", "project": "COMMIT" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    issue["key"].as_str().unwrap().to_string()
}

async fn extra_issue(c: &reqwest::Client, base: &str, auth: &str, title: &str) -> Value {
    c.post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": title, "project": "COMMIT" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn register(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    name: &str,
    session: &str,
    parent: Option<&str>,
) -> Value {
    let mut body = json!({ "agent_name": name, "session_id": session });
    if let Some(parent) = parent {
        body["parent_session_id"] = json!(parent);
    }
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn usage_row(app: &tu::TestApp, session: &str, agent: &str, provider: &str, model: &str) {
    sqlx::query(
        "INSERT INTO usage_sessions
            (session_id, agent_name, provider, model, day, source, imported_at)
         VALUES (?1, ?2, ?3, ?4, '2026-11-09', 'test', '2026-11-09T12:00:00Z')",
    )
    .bind(session)
    .bind(agent)
    .bind(provider)
    .bind(model)
    .execute(&app.db)
    .await
    .unwrap();
}

async fn record(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    body: Value,
) -> (reqwest::StatusCode, Value) {
    let resp = c
        .post(format!("{base}/api/commits"))
        .header("authorization", auth)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let value: Value = resp.json().await.unwrap_or(Value::Null);
    (status, value)
}

fn sha(fill: char) -> String {
    fill.to_string().repeat(40)
}

#[tokio::test]
async fn records_and_resolves_agent_session_orchestrator_and_model() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let key = project_and_issue(&c, &base, &auth).await;
    let orchestrator = "crumb overlord";
    let agent = "plucky brioche";
    let session = "ses_commit_one";
    register(&c, &base, &auth, orchestrator, "ses_orch_one", None).await;
    register(&c, &base, &auth, agent, session, Some("ses_orch_one")).await;
    usage_row(&app, session, agent, "opencode-go", "deepseek-v4.1-flash").await;

    let commit_sha = sha('a');
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": commit_sha,
            "issue": key,
            "agent_name": agent,
            "session_id": session,
            "note": "implemented the thing"
        }),
    )
    .await;
    assert_eq!(status, 201, "record failed: {row}");
    assert_eq!(row["repo"], "Breadway/breadboard");
    assert_eq!(row["sha"], commit_sha);
    assert_eq!(row["issue_key"], key);
    assert_eq!(row["agent_name"], agent);
    assert_eq!(row["session_id"], session);
    assert_eq!(row["orchestrator"], orchestrator);
    assert_eq!(row["orchestrator_session_id"], "ses_orch_one");
    assert_eq!(row["model"], "deepseek-v4.1-flash");
    assert_eq!(row["provider"], "opencode-go");
    assert_eq!(row["model_source"], "session");
    assert_eq!(row["note"], "implemented the thing");

    // The issue's commit list is the roll-up the drawer uses.
    let commits: Vec<Value> = c
        .get(format!("{base}/api/issues/{key}/commits"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["sha"], commit_sha);
    assert_eq!(commits[0]["model"], "deepseek-v4.1-flash");

    // Reverse lookup: SHA to the model and session that wrote it.
    let lookup: Value = c
        .get(format!(
            "{base}/api/commits/lookup?repo=Breadway/breadboard&sha={commit_sha}"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lookup["known"], true);
    assert_eq!(lookup["matched_by"], "exact");
    assert_eq!(lookup["attribution"]["agent_name"], agent);
    assert_eq!(lookup["attribution"]["model"], "deepseek-v4.1-flash");
    assert_eq!(lookup["attribution"]["session_id"], session);

    // The record also appears in the activity timeline.
    let activity: Vec<Value> = c
        .get(format!("{base}/api/issues/{key}/activity"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let commit_event = activity
        .iter()
        .find(|entry| entry["field"] == "commit")
        .expect("commit activity entry missing");
    let summary = commit_event["new_value"].as_str().unwrap();
    assert!(summary.contains("Breadway/breadboard"), "{summary}");
    assert!(summary.contains(agent), "{summary}");
    assert!(summary.contains("deepseek-v4.1-flash"), "{summary}");
}

#[tokio::test]
async fn unknown_models_stay_null_and_an_explicit_report_wins() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let key = project_and_issue(&c, &base, &auth).await;

    let agent = "ambiguity tester";
    let multi = "ses_commit_multi";
    register(&c, &base, &auth, agent, multi, None).await;
    usage_row(&app, multi, agent, "opencode-go", "model-one").await;
    usage_row(&app, multi, agent, "openrouter", "model-two").await;

    // Do not guess between two models used by the same session.
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": sha('b'),
            "issue": key,
            "agent_name": agent,
            "session_id": multi
        }),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert!(row["model"].is_null(), "guessed a model: {row}");
    assert!(row["provider"].is_null(), "guessed a provider: {row}");
    assert_eq!(row["model_source"], "unknown");

    // A session with no usage rows at all is unknown too.
    let quiet = "ses_commit_quiet";
    register(&c, &base, &auth, "quiet agent", quiet, None).await;
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": sha('c'),
            "issue": key,
            "agent_name": "quiet agent",
            "session_id": quiet
        }),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert!(row["model"].is_null());
    assert_eq!(row["model_source"], "unknown");

    // An explicit report is trusted even when the session used several models.
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": sha('d'),
            "issue": key,
            "agent_name": agent,
            "session_id": multi,
            "model": "model-two",
            "provider": "openrouter"
        }),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert_eq!(row["model"], "model-two");
    assert_eq!(row["provider"], "openrouter");
    assert_eq!(row["model_source"], "reported");

    // Re-recording the same SHA without a model must not erase the report.
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": sha('d'),
            "issue": key,
            "agent_name": agent,
            "session_id": multi,
            "note": "amended"
        }),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert_eq!(row["model"], "model-two");
    assert_eq!(row["model_source"], "reported");
    assert_eq!(row["note"], "amended");
}

#[tokio::test]
async fn matches_rebased_and_squashed_commits_and_reports_unknown_plainly() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let key = project_and_issue(&c, &base, &auth).await;

    let agent = "rewrite tracer";
    let session = "ses_commit_rewrite";
    register(&c, &base, &auth, agent, session, None).await;
    usage_row(&app, session, agent, "opencode-go", "deepseek-v4.1-flash").await;

    let original = sha('e');
    let patch_id = sha('f');
    let tree_sha = sha('1');
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": original,
            "issue": key,
            "agent_name": agent,
            "session_id": session,
            "patch_id": patch_id,
            "tree_sha": tree_sha
        }),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert_eq!(row["patch_id"], patch_id);
    assert_eq!(row["tree_sha"], tree_sha);

    // A rebase changes the SHA but keeps the patch id.
    let rebased = sha('2');
    let lookup: Value = c
        .get(format!(
            "{base}/api/commits/lookup?repo=Breadway/breadboard&sha={rebased}&patch_id={patch_id}"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lookup["known"], true);
    assert_eq!(lookup["matched_by"], "patch_id");
    assert_eq!(lookup["attribution"]["sha"], original);
    assert_eq!(lookup["attribution"]["model"], "deepseek-v4.1-flash");

    // A squash produces a new SHA whose tree is the branch tip's tree.
    let squashed = sha('3');
    let lookup: Value = c
        .get(format!(
            "{base}/api/commits/lookup?repo=Breadway/breadboard&sha={squashed}&tree_sha={tree_sha}"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lookup["known"], true);
    assert_eq!(lookup["matched_by"], "tree_sha");
    assert_eq!(lookup["attribution"]["sha"], original);

    // No record and no rewrite hint: say so instead of returning a bare 404.
    let stranger = sha('4');
    let lookup: Value = c
        .get(format!(
            "{base}/api/commits/lookup?repo=Breadway/breadboard&sha={stranger}"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lookup["known"], false);
    assert!(lookup["attribution"].is_null());
    assert_eq!(
        lookup["message"], "no attribution recorded for this commit",
        "{lookup}"
    );

    // Short SHAs and bare repo names are rejected before they reach the table.
    let (status, _) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": "abc1234",
            "issue": key,
            "agent_name": agent,
            "session_id": session
        }),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "breadboard",
            "sha": sha('5'),
            "issue": key,
            "agent_name": agent,
            "session_id": session
        }),
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn pr_card_shows_the_linked_issue_commits() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let target_key = project_and_issue(&c, &base, &auth).await;
    let pr = extra_issue(&c, &base, &auth, "TB-163 implementation").await;
    let pr_id = pr["id"].as_i64().unwrap();
    let pr_key = pr["key"].as_str().unwrap().to_string();
    // Mirror a synced PR card and its closing link to the implementation issue.
    sqlx::query("UPDATE issues SET type = 'pr' WHERE id = ?1")
        .bind(pr_id)
        .execute(&app.db)
        .await
        .unwrap();
    let target: Value = c
        .get(format!("{base}/api/issues/{target_key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let target_id = target["id"].as_i64().unwrap();
    for (issue_id, link_target, kind) in [
        (pr_id, target_id, "closes"),
        (target_id, pr_id, "closed-by"),
    ] {
        sqlx::query("INSERT INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, ?3)")
            .bind(issue_id)
            .bind(link_target)
            .bind(kind)
            .execute(&app.db)
            .await
            .unwrap();
    }

    let agent = "pr tracer";
    let session = "ses_commit_pr";
    register(&c, &base, &auth, agent, session, None).await;
    usage_row(&app, session, agent, "opencode-go", "deepseek-v4.1-flash").await;
    let commit_sha = sha('6');
    let (status, row) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": commit_sha,
            "issue": target_key,
            "agent_name": agent,
            "session_id": session
        }),
    )
    .await;
    assert_eq!(status, 201, "{row}");

    for key in [&pr_key, &target_key] {
        let commits: Vec<Value> = c
            .get(format!("{base}/api/issues/{key}/commits"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(commits.len(), 1, "{key} should see the linked commit");
        assert_eq!(commits[0]["sha"], commit_sha);
    }

    // An unrelated issue does not inherit the commits.
    let other = extra_issue(&c, &base, &auth, "unrelated").await;
    let other_key = other["key"].as_str().unwrap();
    let commits: Vec<Value> = c
        .get(format!("{base}/api/issues/{other_key}/commits"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(commits.is_empty());
}

/// BB-215: a caller that presents a name and a session id that disagree is
/// refused with an explicit conflict, and nothing is recorded. A name with no
/// session id resolves to that name's own registered session instead of
/// borrowing another conversation's.
#[tokio::test]
async fn commit_identity_must_match_the_calling_session() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let key = project_and_issue(&c, &base, &auth).await;
    register(&c, &base, &auth, "alpha", "ses_alpha", None).await;
    register(&c, &base, &auth, "beta", "ses_beta", None).await;

    let refused_sha = sha('3');
    let (status, body) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": refused_sha,
            "issue": key,
            "agent_name": "alpha",
            "session_id": "ses_beta",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CONFLICT, "{body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.contains("ses_beta"), "{message}");
    assert!(message.contains("beta"), "{message}");

    let commits: Vec<Value> = c
        .get(format!("{base}/api/issues/{key}/commits"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(commits.is_empty(), "a refused identity must record nothing");

    let correct_sha = sha('4');
    let (status, body) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": correct_sha,
            "issue": key,
            "agent_name": "alpha",
            "session_id": "ses_alpha",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "{body}");
    assert_eq!(body["agent_name"], "alpha");
    assert_eq!(body["session_id"], "ses_alpha");

    // With no session id the server uses the name's registered session, which
    // is the fallback for clients that cannot send a per-call id.
    let by_name_sha = sha('5');
    let (status, body) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": by_name_sha,
            "issue": key,
            "agent_name": "beta",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "{body}");
    assert_eq!(body["agent_name"], "beta");
    assert_eq!(body["session_id"], "ses_beta");

    // A registration from a process that cannot name its session must not
    // inherit the previous holder's id. Otherwise the name-based fallback
    // keeps borrowing the same stale session that caused the misattribution.
    register(&c, &base, &auth, "delta", "ses_delta", None).await;
    let resp = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "delta", "pid": 4242 }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.text().await.unwrap());
    let stored: Option<String> =
        sqlx::query_scalar("SELECT session_id FROM agent_sessions WHERE agent_name = 'delta'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(stored, None, "a new registration cannot keep a stale session id");

    let no_session_sha = sha('6');
    let (status, body) = record(
        &c,
        &base,
        &auth,
        json!({
            "repo": "Breadway/breadboard",
            "sha": no_session_sha,
            "issue": key,
            "agent_name": "delta",
        }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "{body}");
    assert_eq!(body["agent_name"], "delta");
    assert_eq!(body["session_id"], Value::Null);
}
