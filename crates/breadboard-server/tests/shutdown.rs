#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::Duration;

use breadboard_server::{auth::sha256_hex, db};
use chrono::Utc;
use tempfile::tempdir;
use tokio::time::{sleep, timeout, Instant};

#[tokio::test]
async fn sigterm_exits_promptly_and_closes_sse_streams() {
    let temp = tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let mut child = Command::new(env!("CARGO_BIN_EXE_breadboard-server"))
        .env_clear()
        .env("BREADBOARD_CONFIG", temp.path().join("no-config.toml"))
        .env("BREADBOARD_LISTEN", address.to_string())
        .env("BREADBOARD_DATABASE_PATH", temp.path().join("server.db"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let response = loop {
        match client.get(format!("http://{address}/healthz")).send().await {
            Ok(response) if response.status().is_success() => break response,
            _ if Instant::now() < deadline => sleep(Duration::from_millis(25)).await,
            _ => {
                let _ = child.kill();
                panic!("server did not become healthy");
            }
        }
    };
    drop(response);

    // /events requires auth: mint a bearer token in the server's database.
    let pool = db::connect(&format!(
        "sqlite://{}?mode=rwc",
        temp.path().join("server.db").display()
    ))
    .await
    .unwrap();
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (1, 'tester', 'Tester') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let token = "shutdown-test-token";
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'test', ?2, ?3)",
    )
    .bind(user_id)
    .bind(sha256_hex(token))
    .bind(Utc::now().to_rfc3339())
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let mut events = client
        .get(format!("http://{address}/events"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(events.status().is_success());

    let signal = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success(), "failed to send SIGTERM");

    let status = match timeout(Duration::from_secs(2), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    {
        Ok(status) => status,
        Err(_) => {
            let _ = child.kill();
            panic!("server did not exit promptly after SIGTERM");
        }
    };
    assert!(status.success(), "server exited unsuccessfully: {status}");

    let end = timeout(Duration::from_secs(1), events.chunk())
        .await
        .expect("SSE client stayed open after shutdown")
        .unwrap();
    assert!(
        end.is_none(),
        "SSE client received data after server shutdown"
    );
}
