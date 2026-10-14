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
    // Use LISTEN=...:0 so the server picks an unused port. This eliminates the
    // bind+drop+reuse race that previously existed between the test's std::net::TcpListener
    // and the child's tokio::net::TcpListener (other concurrent tests also bind ephemeral
    // ports; the window allowed EADDRINUSE in the child, causing silent early exit and
    // the opaque "did not become healthy" after 5s with null stderr).
    //
    // Capture stdout/stderr so any startup failure (bind error, migration failure,
    // config panic, etc.) is reported in the test output instead of discarded.
    // Poll child exit during waits. After the server reports healthy we discover
    // the actual bound address from the OS (ss/lsof on the child PID). This is
    // race-free for the purpose of subsequent connections and keeps the same 5s
    // budget for full startup (migrations + bind + first health response).
    let mut child = Command::new(env!("CARGO_BIN_EXE_breadboard-server"))
        .env_clear()
        .env("BREADBOARD_CONFIG", temp.path().join("no-config.toml"))
        .env("BREADBOARD_LISTEN", "127.0.0.1:0")
        .env("BREADBOARD_DATABASE_PATH", temp.path().join("server.db"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Drain pipes into an accumulator for rich diagnostics on any failure path.
    // We do not rely on log parsing for the happy-path address discovery.
    let logs = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stdout_pipe = child.stdout.take().expect("stdout");
    let stderr_pipe = child.stderr.take().expect("stderr");
    let stdout_logs = logs.clone();
    let stdout_handle = std::thread::spawn(move || {
        use std::io::Read;
        let mut reader = stdout_pipe;
        let mut buf = [0; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            stdout_logs
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    let stderr_logs = logs.clone();
    let stderr_handle = std::thread::spawn(move || {
        use std::io::Read;
        let mut reader = stderr_pipe;
        let mut buf = [0; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            stderr_logs
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });

    let captured_logs = || logs.lock().unwrap().clone();

    fn discover_bound_addr(pid: u32) -> Option<std::net::SocketAddr> {
        // Ask the OS which TCP LISTEN socket belongs to this pid.
        // Works on Linux (ss is present on the self-hosted runner and in CI).
        let out = std::process::Command::new("ss")
            .args(["-ltnp"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("LISTEN") && line.contains(&format!("pid={}", pid)) {
                // Typical line contains something like: 127.0.0.1:54321
                for tok in line.split_whitespace() {
                    if tok.contains("127.0.0.1:") {
                        if let Ok(sa) = tok.parse::<std::net::SocketAddr>() {
                            return Some(sa);
                        }
                    }
                }
            }
        }
        None
    }

    let client = reqwest::Client::new();
    let overall_deadline = Instant::now() + Duration::from_secs(5);

    // Wait for health while also checking that the child has not exited.
    // Once /healthz answers 200, discover the bound address from the OS.
    let address: std::net::SocketAddr = loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "server exited early (status={status}) before becoming healthy.\n\
                 captured: {}",
                captured_logs()
            );
        }
        // Try to discover a LISTEN socket for the child. If present, try health.
        if let Some(addr) = discover_bound_addr(child.id()) {
            match client.get(format!("http://{addr}/healthz")).send().await {
                Ok(r) if r.status().is_success() => {
                    // success path: we have a live address
                    break addr;
                }
                _ => {
                    // server bound but not yet serving (or still migrating); keep waiting
                }
            }
        }
        if Instant::now() > overall_deadline {
            let _ = child.kill();
            let _ = child.wait();
            stdout_handle.join().expect("stdout reader panicked");
            stderr_handle.join().expect("stderr reader panicked");
            panic!(
                "server did not become healthy within 5s (no LISTEN socket or /healthz failed).\n\
                 captured: {}",
                captured_logs()
            );
        }
        sleep(Duration::from_millis(25)).await;
    };

    // We reached a healthy server on a discovered address.
    // (The previous response body was dropped by the loop; that's fine.)
    let _ = client.get(format!("http://{address}/healthz")).send().await;

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
    stdout_handle.join().expect("stdout reader panicked");
    stderr_handle.join().expect("stderr reader panicked");
}
