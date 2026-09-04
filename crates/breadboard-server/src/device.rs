//! Device-authorization flow for the CLI/TUI (`breadboard auth login`).
//!
//! `POST /auth/device` → `{ device_code, user_code, verification_uri, ... }`.
//! The user opens `verification_uri` (signing in via Forgejo if needed) and
//! approves; the CLI polls `POST /auth/device/token` until it gets a bearer
//! token, which it then stores and sends as `Authorization: Bearer`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{random_token, sha256_hex, AuthUser};
use crate::error::{AppError, AppResult};
use crate::AppState;

const TTL_MINUTES: i64 = 10;
const POLL_INTERVAL_SECS: u64 = 3;

fn gen_user_code() -> String {
    const ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ23456789"; // no vowels / ambiguous
    let mut rng = rand::thread_rng();
    let pick = |rng: &mut rand::rngs::ThreadRng| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char;
    let a: String = (0..4).map(|_| pick(&mut rng)).collect();
    let b: String = (0..4).map(|_| pick(&mut rng)).collect();
    format!("{a}-{b}")
}

fn normalize_user_code(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

#[derive(Serialize)]
pub struct StartResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: i64,
    interval: u64,
}

pub async fn start(State(app): State<AppState>) -> AppResult<Json<StartResponse>> {
    let device_code = random_token();
    let user_code = gen_user_code();
    let now = Utc::now();
    let expires = now + Duration::minutes(TTL_MINUTES);

    sqlx::query(
        "INSERT INTO device_codes (device_code, user_code, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&device_code)
    .bind(&user_code)
    .bind(now.to_rfc3339())
    .bind(expires.to_rfc3339())
    .execute(&app.db)
    .await?;

    let base = app.config.public_url.trim_end_matches('/');
    Ok(Json(StartResponse {
        verification_uri: format!("{base}/auth/device"),
        verification_uri_complete: format!(
            "{base}/auth/device?user_code={}",
            urlencoding::encode(&user_code)
        ),
        device_code,
        user_code,
        expires_in: TTL_MINUTES * 60,
        interval: POLL_INTERVAL_SECS,
    }))
}

#[derive(Deserialize)]
pub struct PollRequest {
    device_code: String,
}

pub async fn poll(
    State(app): State<AppState>,
    Json(body): Json<PollRequest>,
) -> Response {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT expires_at, token FROM device_codes WHERE device_code = ?1",
    )
    .bind(&body.device_code)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None);

    let Some((expires_at, token)) = row else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "invalid device_code" })))
            .into_response();
    };

    let expired = chrono::DateTime::parse_from_rfc3339(&expires_at)
        .map(|e| e < Utc::now())
        .unwrap_or(true);
    if expired {
        sqlx::query("DELETE FROM device_codes WHERE device_code = ?1")
            .bind(&body.device_code)
            .execute(&app.db)
            .await
            .ok();
        return (StatusCode::GONE, Json(json!({ "error": "expired_token" }))).into_response();
    }

    match token {
        Some(t) => {
            // One-time handoff.
            sqlx::query("DELETE FROM device_codes WHERE device_code = ?1")
                .bind(&body.device_code)
                .execute(&app.db)
                .await
                .ok();
            (StatusCode::OK, Json(json!({ "access_token": t }))).into_response()
        }
        None => (
            StatusCode::ACCEPTED,
            Json(json!({ "status": "authorization_pending" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    user_code: Option<String>,
}

/// The approval page. Client-side: if not signed in, bounce through Forgejo
/// OAuth and come back here.
pub async fn page(Query(q): Query<PageQuery>) -> Html<String> {
    let prefill = q.user_code.unwrap_or_default();
    Html(PAGE_HTML.replace("{{PREFILL}}", &html_escape(&prefill)))
}

#[derive(Deserialize)]
pub struct ApproveRequest {
    user_code: String,
}

pub async fn approve(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<ApproveRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let code = normalize_user_code(&body.user_code);
    let code = if code.len() == 8 {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code
    };

    let row: Option<(String, Option<i64>)> = sqlx::query_as(
        "SELECT expires_at, approved_by FROM device_codes WHERE user_code = ?1",
    )
    .bind(&code)
    .fetch_optional(&app.db)
    .await?;
    let Some((expires_at, approved_by)) = row else {
        return Err(AppError::bad_request("unknown code"));
    };
    if chrono::DateTime::parse_from_rfc3339(&expires_at)
        .map(|e| e < Utc::now())
        .unwrap_or(true)
    {
        return Err(AppError::bad_request("this code has expired"));
    }
    if approved_by.is_some() {
        return Ok(Json(json!({ "status": "already_approved" })));
    }

    let token = random_token();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'cli', ?2, ?3)",
    )
    .bind(user.0.id)
    .bind(sha256_hex(&token))
    .bind(Utc::now().to_rfc3339())
    .execute(&app.db)
    .await?;

    sqlx::query("UPDATE device_codes SET approved_by = ?2, token = ?3 WHERE user_code = ?1")
        .bind(&code)
        .bind(user.0.id)
        .bind(&token)
        .execute(&app.db)
        .await?;

    Ok(Json(json!({ "status": "approved" })))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PAGE_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>breadboard · authorize device</title>
<style>
  :root { color-scheme: dark; }
  body { margin:0; min-height:100vh; display:grid; place-items:center;
    background:#0c0c0c; color:#e6e6e6;
    font:14px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }
  .card { background:#161616; border:1px solid #2a2a2a; border-radius:10px;
    padding:28px 32px; width:min(400px,90vw); text-align:center; }
  h1 { margin:0 0 4px; font-size:18px; }
  p { color:#8a8a8a; }
  .code { font:600 26px/1 ui-monospace,monospace; letter-spacing:3px;
    background:#0c0c0c; border:1px solid #2a2a2a; border-radius:8px;
    padding:14px; margin:16px 0; }
  input { font:inherit; text-align:center; letter-spacing:2px; text-transform:uppercase;
    background:#0c0c0c; color:#e6e6e6; border:1px solid #2a2a2a; border-radius:6px;
    padding:10px; width:100%; box-sizing:border-box; }
  button { font:inherit; font-weight:600; background:#d98a3d; color:#000; border:none;
    border-radius:6px; padding:10px 18px; margin-top:14px; cursor:pointer; width:100%; }
  .msg { margin-top:14px; min-height:20px; }
  .ok { color:#4caf50; } .err { color:#e05252; }
</style>
</head>
<body>
<div class="card">
  <h1>Authorize a device</h1>
  <p>A <code>breadboard</code> CLI is asking to sign in as you.</p>
  <div id="prompt">
    <p>Confirm the code shown in your terminal:</p>
    <input id="code" value="{{PREFILL}}" placeholder="XXXX-XXXX" autocomplete="off">
    <button id="go">Authorize</button>
  </div>
  <div class="msg" id="msg"></div>
</div>
<script>
const msg = document.getElementById('msg');
async function ensureAuth() {
  const r = await fetch('/api/me', { credentials: 'include' });
  if (r.status === 401) {
    location.href = '/auth/login?next=' + encodeURIComponent(location.pathname + location.search);
    return false;
  }
  return true;
}
document.getElementById('go').addEventListener('click', async () => {
  const code = document.getElementById('code').value.trim();
  if (!code) return;
  const r = await fetch('/auth/device/approve', {
    method: 'POST', credentials: 'include',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ user_code: code }),
  });
  const j = await r.json().catch(() => ({}));
  if (r.ok) {
    document.getElementById('prompt').style.display = 'none';
    msg.className = 'msg ok';
    msg.textContent = j.status === 'already_approved'
      ? 'Already authorized — return to your terminal.'
      : 'Device authorized. Return to your terminal.';
  } else {
    msg.className = 'msg err';
    msg.textContent = j.error || 'Could not authorize that code.';
  }
});
ensureAuth();
</script>
</body>
</html>
"#;
