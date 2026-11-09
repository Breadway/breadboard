//! Typed issue artifacts: upload, immutability, listing, download and expiry.

mod common;
use base64::Engine as _;
use breadboard_server::artifacts;
use chrono::{Duration, Utc};
use common as tu;
use serde_json::{json, Value};

async fn setup() -> (tu::TestApp, reqwest::Client, String, String, String) {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    c.post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "BREAD", "name": "bread" }))
        .send()
        .await
        .unwrap();
    let issue: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "evidence", "project": "BREAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = issue["key"].as_str().unwrap().to_string();
    (app, c, base, auth, key)
}

fn png_bytes() -> String {
    base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfake image body")
}

#[tokio::test]
async fn upload_list_download_and_immutability() {
    let (app, c, base, auth, key) = setup().await;

    let created: Value = c
        .post(format!("{base}/api/issues/{key}/artifacts/base64"))
        .header("authorization", &auth)
        .json(&json!({
            "filename": "baseline.png",
            "mime": "image/png",
            "kind": "screenshot",
            "data_base64": png_bytes(),
            "agent_name": "doughy-rye",
            "session_id": "ses_artifact_test",
            "review_ref": "PR-7"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(created["kind"], "screenshot");
    assert_eq!(created["uploader_agent"], "doughy-rye");
    assert_eq!(created["session_id"], "ses_artifact_test");
    assert_eq!(created["review_ref"], "PR-7");
    assert_eq!(created["mime"], "image/png");
    assert!(created["expires_at"].as_str().unwrap() > created["created_at"].as_str().unwrap());
    let id = created["id"].as_i64().unwrap();

    let listed: Vec<Value> = c
        .get(format!("{base}/api/issues/{key}/artifacts"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], id);

    let download = c
        .get(format!("{base}/api/artifacts/{id}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(
        download.headers()["content-type"].to_str().unwrap(),
        "image/png"
    );
    assert_eq!(
        download.headers()["x-content-type-options"]
            .to_str()
            .unwrap(),
        "nosniff"
    );

    // The row is immutable at the database level, not merely by API.
    let update = sqlx::query("UPDATE issue_artifacts SET kind = 'diff' WHERE id = ?1")
        .bind(id)
        .execute(&app.db)
        .await;
    assert!(
        update.is_err(),
        "artifact row update unexpectedly succeeded"
    );

    // An unknown comment id cannot be attached.
    let bad_comment = c
        .post(format!("{base}/api/issues/{key}/artifacts/base64"))
        .header("authorization", &auth)
        .json(&json!({
            "filename": "x.png",
            "mime": "image/png",
            "kind": "screenshot",
            "data_base64": png_bytes(),
            "comment_id": 999999
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_comment.status(), 400);

    // A screenshot must actually look like an image.
    let bad_mime = c
        .post(format!("{base}/api/issues/{key}/artifacts/base64"))
        .header("authorization", &auth)
        .json(&json!({
            "filename": "x.txt",
            "mime": "text/plain",
            "kind": "screenshot",
            "data_base64": png_bytes()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_mime.status(), 400);
}

#[tokio::test]
async fn multipart_upload_records_human_uploader() {
    let (_app, c, base, auth, key) = setup().await;

    let boundary = "----bb160test";
    let mut body = String::new();
    body.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\ndiff\r\n"
    ));
    body.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"change.patch\"\r\nContent-Type: text/x-diff\r\n\r\n--- a\n+++ b\n"
    ));
    body.push_str(&format!("\r\n--{boundary}--\r\n"));

    let created: Value = c
        .post(format!("{base}/api/issues/{key}/artifacts"))
        .header("authorization", &auth)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["kind"], "diff");
    assert_eq!(created["uploader_agent"], "human:tester");
    assert!(created["session_id"].is_null());

    // Missing kind is refused.
    let boundary = "----bb160test2";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.diff\"\r\nContent-Type: text/x-diff\r\n\r\nx\r\n--{boundary}--\r\n"
    );
    let missing = c
        .post(format!("{base}/api/issues/{key}/artifacts"))
        .header("authorization", &auth)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 400);
}

#[tokio::test]
async fn expired_artifacts_are_hidden_gone_and_purged() {
    let (app, c, base, auth, key) = setup().await;
    let issue_id: i64 = sqlx::query_scalar("SELECT id FROM issues WHERE key = ?1")
        .bind(&key)
        .fetch_one(&app.db)
        .await
        .unwrap();

    let stored = "artifacts/expired-test.bin";
    let dir = app.state.config.uploads_dir().join("artifacts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = app.state.config.uploads_dir().join(stored);
    std::fs::write(&path, b"old evidence").unwrap();

    let now = Utc::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO issue_artifacts
            (issue_id, kind, filename, mime, size, path, uploader_agent, created_at, expires_at)
         VALUES (?1, 'other', 'old.bin', 'application/octet-stream', 12, ?2, 'human:tester', ?3, ?4)
         RETURNING id",
    )
    .bind(issue_id)
    .bind(stored)
    .bind(now - Duration::days(100))
    .bind(now - Duration::days(1))
    .fetch_one(&app.db)
    .await
    .unwrap();

    let listed: Vec<Value> = c
        .get(format!("{base}/api/issues/{key}/artifacts"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(listed.is_empty(), "expired artifact should not be listed");

    let download = c
        .get(format!("{base}/api/artifacts/{id}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 410);

    let purged = artifacts::purge_expired(&app.state).await.unwrap();
    assert_eq!(purged, 1);
    assert!(!path.exists(), "expired artifact file should be removed");
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issue_artifacts WHERE id = ?1")
        .bind(id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn limits_and_retention_are_explicit() {
    use breadboard_core::ArtifactKind;
    assert_eq!(
        artifacts::kind_limit(ArtifactKind::Screenshot),
        10 * 1024 * 1024
    );
    assert_eq!(artifacts::kind_limit(ArtifactKind::Diff), 25 * 1024 * 1024);
    assert_eq!(
        artifacts::kind_limit(ArtifactKind::CiLog),
        artifacts::MAX_ARTIFACT_BYTES
    );
    assert_eq!(artifacts::retention(ArtifactKind::CiLog).num_days(), 30);
    assert_eq!(
        artifacts::retention(ArtifactKind::Screenshot).num_days(),
        180
    );
}
