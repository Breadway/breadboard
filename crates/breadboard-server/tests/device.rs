//! Device-authorization flow: start → (approve as an authed user) → poll → token.

mod common;

use serde_json::Value;

#[tokio::test]
async fn device_flow_end_to_end() {
    let app = common::spawn().await;
    let base = format!("http://{}", app.addr);
    let c = reqwest::Client::new();

    // 1. CLI starts the flow.
    let start: Value = c
        .post(format!("{base}/auth/device"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = start["device_code"].as_str().unwrap().to_string();
    let user_code = start["user_code"].as_str().unwrap().to_string();
    assert!(start["verification_uri_complete"]
        .as_str()
        .unwrap()
        .contains(&*urlencoding::encode(&user_code)));

    // 2. Poll before approval -> pending (202).
    let pending = c
        .post(format!("{base}/auth/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(pending.status(), 202);

    // 3. A signed-in user approves (bearer auth stands in for the web session).
    let approve = c
        .post(format!("{base}/auth/device/approve"))
        .bearer_auth(&app.token)
        .json(&serde_json::json!({ "user_code": user_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200);

    // 4. Poll again -> the bearer token, one-time.
    let approved: Value = c
        .post(format!("{base}/auth/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = approved["access_token"].as_str().unwrap().to_string();

    // second poll for the same device_code is now gone (410).
    let gone = c
        .post(format!("{base}/auth/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);

    // 5. The minted token authenticates.
    let me = c
        .get(format!("{base}/api/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
}

#[tokio::test]
async fn device_approve_requires_auth() {
    let app = common::spawn().await;
    let base = format!("http://{}", app.addr);
    let c = reqwest::Client::new();

    let start: Value = c
        .post(format!("{base}/auth/device"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let unauth = c
        .post(format!("{base}/auth/device/approve"))
        .json(&serde_json::json!({ "user_code": start["user_code"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
}
