//! Authentication.
//!
//! - **Web**: Forgejo OAuth2 (`/auth/login` → `/auth/callback`) → opaque session
//!   token in an httponly cookie, hashed into the `sessions` table.
//! - **CLI/TUI**: `Authorization: Bearer <token>` checked against `api_tokens`.
//! - **Local dev**: `POST /auth/dev-login` when `BREADBOARD_DEV_LOGIN=1`, so the
//!   API is testable without a registered OAuth app. Never enable in production.

use axum::extract::{FromRef, FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Json, RequestPartsExt};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use breadboard_core::User;
use chrono::{Duration, Utc};
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use time::Duration as CookieDuration;

use crate::error::{AppError, AppResult};
use crate::{store, AppState};

pub const SESSION_COOKIE: &str = "bb_session";
const STATE_COOKIE: &str = "bb_oauth_state";
const NEXT_COOKIE: &str = "bb_oauth_next";
const SESSION_TTL_DAYS: i64 = 30;

/// Only accept a same-origin path as a post-login redirect target.
fn safe_next(next: Option<&str>) -> Option<String> {
    let n = next?;
    (n.starts_with('/') && !n.starts_with("//")).then(|| n.to_string())
}

pub fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex(&buf)
}

pub fn dev_login_enabled() -> bool {
    std::env::var("BREADBOARD_DEV_LOGIN").as_deref() == Ok("1")
}

// ---------------------------------------------------------------------------
// Extractor
// ---------------------------------------------------------------------------

/// An authenticated request. Rejects with 401 if there is no valid session
/// cookie or bearer token.
pub struct AuthUser(pub User);

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);

        // 1. Bearer token (CLI/TUI).
        if let Some(value) = parts.headers.get(header::AUTHORIZATION) {
            if let Some(token) = value.to_str().ok().and_then(|v| v.strip_prefix("Bearer ")) {
                let hash = sha256_hex(token.trim());
                let user_id: Option<i64> =
                    sqlx::query_scalar("SELECT user_id FROM api_tokens WHERE token_hash = ?1")
                        .bind(&hash)
                        .fetch_optional(&app.db)
                        .await?;
                if let Some(uid) = user_id {
                    sqlx::query("UPDATE api_tokens SET last_used = ?2 WHERE token_hash = ?1")
                        .bind(&hash)
                        .bind(Utc::now().to_rfc3339())
                        .execute(&app.db)
                        .await
                        .ok();
                    return Ok(AuthUser(store::get_user(&app.db, uid).await?));
                }
                return Err(AppError::Unauthorized);
            }
        }

        // 2. Session cookie (web).
        let jar = parts.extract::<CookieJar>().await.unwrap_or_default();
        if let Some(cookie) = jar.get(SESSION_COOKIE) {
            let hash = sha256_hex(cookie.value());
            let row: Option<(i64, String)> =
                sqlx::query_as("SELECT user_id, expires_at FROM sessions WHERE token_hash = ?1")
                    .bind(&hash)
                    .fetch_optional(&app.db)
                    .await?;
            if let Some((uid, expires_at)) = row {
                let live = chrono::DateTime::parse_from_rfc3339(&expires_at)
                    .map(|e| e > Utc::now())
                    .unwrap_or(false);
                if live {
                    return Ok(AuthUser(store::get_user(&app.db, uid).await?));
                }
                sqlx::query("DELETE FROM sessions WHERE token_hash = ?1")
                    .bind(&hash)
                    .execute(&app.db)
                    .await
                    .ok();
            }
        }

        Err(AppError::Unauthorized)
    }
}

async fn create_session(app: &AppState, user_id: i64) -> AppResult<Cookie<'static>> {
    let token = random_token();
    let now = Utc::now();
    let expires = now + Duration::days(SESSION_TTL_DAYS);
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(sha256_hex(&token))
    .bind(user_id)
    .bind(now.to_rfc3339())
    .bind(expires.to_rfc3339())
    .execute(&app.db)
    .await?;

    let secure = app.config.public_url.starts_with("https://");
    let cookie = Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(secure)
        .max_age(CookieDuration::days(SESSION_TTL_DAYS))
        .build();
    Ok(cookie)
}

fn clear_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, "")).path("/").build()
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    next: Option<String>,
}

pub async fn login(
    State(app): State<AppState>,
    jar: CookieJar,
    Query(q): Query<LoginQuery>,
) -> AppResult<Response> {
    let fg = &app.config.forgejo;
    if fg.oauth_client_id.is_empty() {
        return Err(AppError::bad_request(
            "Forgejo OAuth is not configured (forgejo.oauth_client_id)",
        ));
    }
    let state = random_token();
    let redirect_uri = format!("{}/auth/callback", app.config.public_url);
    let url = format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&state={}",
        fg.base_url.trim_end_matches('/'),
        urlencoding::encode(&fg.oauth_client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&state),
    );

    let state_cookie = Cookie::build((STATE_COOKIE, state))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(CookieDuration::minutes(10))
        .build();

    let mut jar = jar.add(state_cookie);
    if let Some(next) = safe_next(q.next.as_deref()) {
        jar = jar.add(
            Cookie::build((NEXT_COOKIE, next))
                .http_only(true)
                .same_site(SameSite::Lax)
                .path("/")
                .max_age(CookieDuration::minutes(10))
                .build(),
        );
    }

    Ok((jar, Redirect::to(&url)).into_response())
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct ForgejoUser {
    id: i64,
    login: String,
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

pub async fn callback(
    State(app): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> AppResult<Response> {
    let expected = jar.get(STATE_COOKIE).map(|c| c.value().to_string());
    if expected.as_deref() != Some(q.state.as_str()) {
        return Err(AppError::bad_request("OAuth state mismatch"));
    }

    let fg = &app.config.forgejo;
    let redirect_uri = format!("{}/auth/callback", app.config.public_url);
    let token: TokenResponse = app
        .http
        .post(format!(
            "{}/login/oauth/access_token",
            fg.base_url.trim_end_matches('/')
        ))
        .json(&json!({
            "client_id": fg.oauth_client_id,
            "client_secret": fg.oauth_client_secret,
            "code": q.code,
            "grant_type": "authorization_code",
            "redirect_uri": redirect_uri,
        }))
        .send()
        .await
        .map_err(|e| AppError::Other(e.into()))?
        .error_for_status()
        .map_err(|e| AppError::Other(e.into()))?
        .json()
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let fg_user: ForgejoUser = app
        .http
        .get(format!("{}/api/v1/user", fg.base_url.trim_end_matches('/')))
        .bearer_auth(&token.access_token)
        .send()
        .await
        .map_err(|e| AppError::Other(e.into()))?
        .error_for_status()
        .map_err(|e| AppError::Other(e.into()))?
        .json()
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let display = if fg_user.full_name.trim().is_empty() {
        fg_user.login.clone()
    } else {
        fg_user.full_name.clone()
    };
    let user = store::upsert_user(
        &app.db,
        fg_user.id,
        &fg_user.login,
        &display,
        fg_user.avatar_url.as_deref(),
    )
    .await?;

    let cookie = create_session(&app, user.id).await?;
    let next = jar
        .get(NEXT_COOKIE)
        .and_then(|c| safe_next(Some(c.value())))
        .unwrap_or_else(|| "/".to_string());
    let jar = jar
        .remove(clear_cookie(STATE_COOKIE))
        .remove(clear_cookie(NEXT_COOKIE))
        .add(cookie);
    Ok((jar, Redirect::to(&next)).into_response())
}

pub async fn logout(State(app): State<AppState>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?1")
            .bind(sha256_hex(cookie.value()))
            .execute(&app.db)
            .await
            .ok();
    }
    (
        jar.remove(clear_cookie(SESSION_COOKIE)),
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct DevLogin {
    login: String,
    #[serde(default)]
    name: Option<String>,
}

pub async fn dev_login(
    State(app): State<AppState>,
    jar: CookieJar,
    Json(body): Json<DevLogin>,
) -> AppResult<Response> {
    if !dev_login_enabled() {
        return Err(AppError::Forbidden);
    }
    // Deterministic fake forgejo id from the login.
    let fake_id = -(i64::from(crc32(&body.login)) & 0x7fff_ffff);
    let name = body.name.unwrap_or_else(|| body.login.clone());
    let user = store::upsert_user(&app.db, fake_id, &body.login, &name, None).await?;
    let cookie = create_session(&app, user.id).await?;
    Ok((jar.add(cookie), Json(user)).into_response())
}

pub async fn me(user: AuthUser) -> Json<User> {
    Json(user.0)
}

/// Tiny CRC32 (dev-login only; not security-sensitive).
fn crc32(s: &str) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for byte in s.bytes() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
