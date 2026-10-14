#![cfg(unix)]

//! SIGTERM must stop the server promptly and end open SSE streams.
//!
//! Timing budgets, none of which may be loosened without new evidence:
//! - `STARTUP_BUDGET` (5s): from spawn until the child has logged its bound
//!   address AND answered `/healthz`. Discovery and every HTTP probe share this
//!   one deadline.
//! - `SIGTERM_BUDGET` (2s): from SIGTERM until the child exits successfully.
//! - `SSE_EOF_BUDGET` (1s): from child exit until the SSE client sees EOF.
//!
//! `SETUP_BUDGET` is separate and only covers creating and migrating the
//! database before the child is spawned. A cold SQLite file costs about 21
//! fsyncs (WAL switch, six migrations, seed), so on a runner whose disk syncs
//! slowly that work dominates and has nothing to do with shutdown. Evidence:
//! with the previous test, 300ms per fsync in the child reproduced CI run 72
//! exactly (only the `starting` log line, failure at 5.0s), while a warm start
//! costs about one fsync and took 22ms even at 200ms per fsync. In run 72 the
//! sibling tests that do the same connect and migrate took 12s to 40s, where
//! they take under a second locally. Doing the cold work up front means the
//! child's 5s budget measures a warm start. The setup budget is bounded and
//! generous because it only guards against a hang, not against slow disks.
//!
//! The child's startup stages are logged with `elapsed_ms`, and the test prints
//! the child's complete output on every failure path, so a slow stage is
//! visible in the failure report.

use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use breadboard_server::{auth::sha256_hex, db, store};
use chrono::Utc;
use tempfile::tempdir;
use tokio::time::{sleep, timeout, timeout_at, Instant};

const STARTUP_BUDGET: Duration = Duration::from_secs(5);
const SETUP_BUDGET: Duration = Duration::from_secs(60);
const SIGTERM_BUDGET: Duration = Duration::from_secs(2);
const SSE_EOF_BUDGET: Duration = Duration::from_secs(1);
const PROBE_INTERVAL: Duration = Duration::from_millis(25);
const READER_JOIN_BUDGET: Duration = Duration::from_secs(2);
const CAPTURE_LIMIT: usize = 256 * 1024;

/// Bounded, shared buffer for the child's stdout and stderr.
#[derive(Default)]
struct Capture {
    data: Mutex<Vec<u8>>,
    dropped: Mutex<usize>,
}

impl Capture {
    fn push(&self, chunk: &[u8]) {
        let mut data = self.data.lock().unwrap();
        let room = CAPTURE_LIMIT.saturating_sub(data.len());
        let take = room.min(chunk.len());
        data.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            *self.dropped.lock().unwrap() += chunk.len() - take;
        }
    }

    fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.data.lock().unwrap()).into_owned();
        let dropped = *self.dropped.lock().unwrap();
        if dropped > 0 {
            text.push_str(&format!("\n[... {dropped} more bytes not captured]"));
        }
        text
    }
}

struct Reader {
    handle: JoinHandle<()>,
    done: Arc<AtomicBool>,
}

fn spawn_reader<R: Read + Send + 'static>(mut pipe: R, capture: Arc<Capture>) -> Reader {
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => capture.push(&buf[..n]),
            }
        }
        flag.store(true, Ordering::SeqCst);
    });
    Reader { handle, done }
}

/// Owns the child process. Dropping it (including during a panic unwind)
/// kills the child if it is still running, reaps it, and joins the output
/// readers, so no path leaves a live or zombie server behind.
struct ChildGuard {
    child: Child,
    readers: Vec<Reader>,
    capture: Arc<Capture>,
}

impl ChildGuard {
    fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let capture = Arc::new(Capture::default());
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            readers.push(spawn_reader(out, capture.clone()));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(spawn_reader(err, capture.clone()));
        }
        Ok(Self {
            child,
            readers,
            capture,
        })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn logs(&self) -> String {
        self.capture.text()
    }

    /// Kill if still running, wait, and join the readers (bounded, since a
    /// descendant that inherited the pipes could keep them open). Idempotent.
    fn reap(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let give_up = std::time::Instant::now() + READER_JOIN_BUDGET;
        for reader in self.readers.drain(..) {
            while !reader.done.load(Ordering::SeqCst) && std::time::Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(5));
            }
            if reader.done.load(Ordering::SeqCst) {
                let _ = reader.handle.join();
            }
        }
    }

    /// Reap, then return `msg` followed by the complete captured output.
    fn failure(&mut self, msg: String) -> String {
        self.reap();
        format!("{msg}\ncaptured child output:\n{}", self.logs())
    }

    /// Send SIGTERM with `program` (normally `kill`).
    fn send_sigterm_with(&self, program: &str) -> Result<(), String> {
        let status = Command::new(program)
            .args(["-TERM", &self.id().to_string()])
            .status()
            .map_err(|e| format!("could not run {program}: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{program} -TERM failed: {status}"))
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.reap();
        if std::thread::panicking() {
            eprintln!("captured child output:\n{}", self.logs());
        }
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Find the address in the server's `listening requested=.. bound=..` line.
fn parse_bound_addr(logs: &str) -> Option<SocketAddr> {
    strip_ansi(logs)
        .lines()
        .filter(|line| line.contains("listening"))
        .flat_map(|line| line.split_whitespace())
        .find_map(|token| token.strip_prefix("bound=")?.parse().ok())
}

/// Wait for the child to log its bound address and answer `/healthz`. The log
/// scan and every probe share the single deadline `started + budget`.
async fn wait_until_healthy(
    guard: &mut ChildGuard,
    client: &reqwest::Client,
    started: Instant,
    budget: Duration,
) -> Result<SocketAddr, String> {
    let deadline = started + budget;
    let mut addr: Option<SocketAddr> = None;
    loop {
        match guard.try_wait() {
            Ok(Some(status)) => {
                return Err(guard.failure(format!(
                    "server exited early ({status}) before becoming healthy"
                )));
            }
            Ok(None) => {}
            Err(e) => return Err(guard.failure(format!("try_wait failed: {e}"))),
        }

        if addr.is_none() {
            addr = parse_bound_addr(&guard.logs());
        }
        if let Some(addr) = addr {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                let probe = timeout_at(
                    deadline,
                    client
                        .get(format!("http://{addr}/healthz"))
                        .timeout(remaining)
                        .send(),
                )
                .await;
                if let Ok(Ok(resp)) = probe {
                    if resp.status().is_success() && Instant::now() <= deadline {
                        return Ok(addr);
                    }
                }
            }
        }

        let now = Instant::now();
        if now >= deadline {
            let stage = match addr {
                Some(a) => format!("child logged bound={a} but /healthz did not succeed"),
                None => "child never logged its bound address".to_string(),
            };
            return Err(guard.failure(format!(
                "server did not become healthy within {budget:?} (elapsed {:?}): {stage}",
                now - started
            )));
        }
        sleep(PROBE_INTERVAL.min(deadline - now)).await;
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Create, migrate and seed the database and mint the bearer token the SSE
/// request needs, all before the child starts (see the module docs).
async fn prepare_database(db_path: &Path, token: &str) {
    timeout(SETUP_BUDGET, async {
        let pool = db::connect(&format!("sqlite://{}?mode=rwc", db_path.display()))
            .await
            .unwrap();
        store::ensure_seed(&pool).await.unwrap();
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (forgejo_id, login, display_name) VALUES (1, 'tester', 'Tester') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
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
    })
    .await
    .unwrap_or_else(|_| panic!("database setup exceeded {SETUP_BUDGET:?}"));
}

#[tokio::test]
async fn sigterm_exits_promptly_and_closes_sse_streams() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("server.db");
    let token = "shutdown-test-token";
    prepare_database(&db_path, token).await;

    // BREADBOARD_LISTEN=...:0 lets the OS pick the port, so there is no
    // reserve-release-rebind window. The child logs the bound address and the
    // test reads it from that line.
    let started = Instant::now();
    let mut guard = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_breadboard-server"))
            .env_clear()
            .env("BREADBOARD_CONFIG", temp.path().join("no-config.toml"))
            .env("BREADBOARD_LISTEN", "127.0.0.1:0")
            .env("BREADBOARD_DATABASE_PATH", &db_path),
    )
    .unwrap();

    let client = http_client();
    let address = wait_until_healthy(&mut guard, &client, started, STARTUP_BUDGET)
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    let mut events = client
        .get(format!("http://{address}/events"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(events.status().is_success());

    guard
        .send_sigterm_with("kill")
        .unwrap_or_else(|e| panic!("failed to send SIGTERM: {e}"));

    let term_deadline = Instant::now() + SIGTERM_BUDGET;
    let status = loop {
        if let Some(status) = guard.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= term_deadline {
            panic!("server did not exit within {SIGTERM_BUDGET:?} after SIGTERM");
        }
        sleep(PROBE_INTERVAL).await;
    };
    assert!(status.success(), "server exited unsuccessfully: {status}");

    let end = timeout(SSE_EOF_BUDGET, events.chunk())
        .await
        .expect("SSE client stayed open after shutdown")
        .unwrap();
    assert!(
        end.is_none(),
        "SSE client received data after server shutdown"
    );

    guard.reap();
}

// Failure-injection coverage for the harness itself. These use stand-in
// children (`sh`) so each failure mode is deterministic.

fn sh(script: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", script]);
    cmd
}

/// `kill -0` succeeds for a live process and for an unreaped zombie, so a
/// false result means the pid was killed and reaped.
fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn parses_bound_address_from_plain_and_ansi_log_lines() {
    let plain = "x INFO breadboard_server: listening requested=127.0.0.1:0 bound=127.0.0.1:35593 elapsed_ms=71\n";
    assert_eq!(
        parse_bound_addr(plain),
        Some("127.0.0.1:35593".parse().unwrap())
    );
    let ansi = "\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \u{1b}[2mbreadboard_server\u{1b}[0m\u{1b}[2m:\u{1b}[0m listening \u{1b}[3mrequested\u{1b}[0m\u{1b}[2m=\u{1b}[0m127.0.0.1:0 bound=127.0.0.1:4242\n";
    assert_eq!(
        parse_bound_addr(ansi),
        Some("127.0.0.1:4242".parse().unwrap())
    );
    // A bind in some other message must not be mistaken for the listener line.
    assert_eq!(parse_bound_addr("starting listen=127.0.0.1:0\n"), None);
    assert_eq!(parse_bound_addr("other bound=127.0.0.1:9\n"), None);
}

#[tokio::test]
async fn slow_discovery_fails_at_the_deadline_instead_of_passing_late() {
    // The stand-in only reports its address after 8s; the budget is 1s.
    let mut guard = ChildGuard::spawn(&mut sh(
        "i=0; while [ $i -lt 40 ]; do sleep 0.2; i=$((i+1)); done; echo 'listening requested=127.0.0.1:0 bound=127.0.0.1:1'; exec sleep 30",
    ))
    .unwrap();
    let pid = guard.id();
    let begin = Instant::now();
    let err = wait_until_healthy(&mut guard, &http_client(), begin, Duration::from_secs(1))
        .await
        .expect_err("a child that reports its address after the deadline must fail");
    let elapsed = begin.elapsed();
    assert!(
        elapsed >= Duration::from_secs(1),
        "gave up early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "deadline not honored, took {elapsed:?}"
    );
    assert!(err.contains("never logged its bound address"), "{err}");
    assert!(!process_exists(pid), "child left running or unreaped");
}

#[tokio::test]
async fn hung_health_probe_cannot_outlive_the_deadline() {
    // Accepts connections but never answers, so every probe would hang.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = silent.local_addr().unwrap();
    let mut guard = ChildGuard::spawn(&mut sh(&format!(
        "echo 'listening requested=127.0.0.1:0 bound={addr}'; exec sleep 30"
    )))
    .unwrap();
    let pid = guard.id();
    let begin = Instant::now();
    let err = wait_until_healthy(&mut guard, &http_client(), begin, Duration::from_secs(1))
        .await
        .expect_err("a hung /healthz must fail");
    let elapsed = begin.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "probe outlived the deadline, took {elapsed:?}"
    );
    assert!(err.contains("/healthz did not succeed"), "{err}");
    assert!(!process_exists(pid), "child left running or unreaped");
    drop(silent);
}

#[tokio::test]
async fn early_exit_reports_status_and_complete_output() {
    let mut guard =
        ChildGuard::spawn(&mut sh("echo from-stdout; echo from-stderr >&2; exit 3")).unwrap();
    let begin = Instant::now();
    let err = wait_until_healthy(&mut guard, &http_client(), begin, Duration::from_secs(5))
        .await
        .expect_err("an exited child is not healthy");
    assert!(err.contains("exit status: 3"), "{err}");
    assert!(err.contains("from-stdout"), "{err}");
    assert!(err.contains("from-stderr"), "{err}");
    assert!(begin.elapsed() < Duration::from_secs(3));
}

#[test]
fn failing_sigterm_command_still_reaps_the_child() {
    let pid;
    {
        let guard = ChildGuard::spawn(&mut sh("exec sleep 60")).unwrap();
        pid = guard.id();
        assert!(process_exists(pid));
        assert!(guard.send_sigterm_with("false").is_err());
        assert!(guard
            .send_sigterm_with("breadboard-no-such-program")
            .is_err());
        assert!(process_exists(pid), "child should still be running here");
    }
    assert!(!process_exists(pid), "guard drop left the child alive");
}

#[test]
fn panic_unwind_reaps_the_child() {
    let pid = Arc::new(Mutex::new(0u32));
    let seen = pid.clone();
    let result = std::panic::catch_unwind(move || {
        let guard = ChildGuard::spawn(&mut sh("exec sleep 60")).unwrap();
        *seen.lock().unwrap() = guard.id();
        panic!("injected failure while the child is running");
    });
    assert!(result.is_err());
    let pid = *pid.lock().unwrap();
    assert_ne!(pid, 0);
    assert!(!process_exists(pid), "unwind left the child alive");
}

#[test]
fn output_capture_is_bounded_and_does_not_block_the_child() {
    let mut guard = ChildGuard::spawn(&mut sh(
        "i=0; while [ $i -lt 2000 ]; do printf '%0200d\\n' 0; i=$((i+1)); done; echo done-marker >&2",
    ))
    .unwrap();
    let status = loop {
        if let Some(status) = guard.try_wait().unwrap() {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    guard.reap();
    let logs = guard.logs();
    assert!(logs.contains("more bytes not captured"), "not truncated");
    assert!(logs.len() < CAPTURE_LIMIT + 100);
}
