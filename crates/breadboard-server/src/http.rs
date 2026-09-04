//! HTTP surface: health, SSE, auth, and the `/api` REST routes.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use futures::stream::Stream;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::{api, auth, device, webhook, AppState};

pub fn router(state: AppState) -> Router {
    let api_routes = Router::new()
        .route("/me", get(auth::me))
        .route("/projects", get(api::list_projects).post(api::create_project))
        .route(
            "/projects/{id}",
            get(api::get_project).patch(api::update_project),
        )
        .route("/epics", get(api::list_epics).post(api::create_epic))
        .route(
            "/epics/{id}",
            get(api::get_epic)
                .patch(api::update_epic)
                .delete(api::delete_epic),
        )
        .route("/cycles", get(api::list_cycles).post(api::create_cycle))
        .route("/cycles/{id}/start", post(api::start_cycle))
        .route("/cycles/{id}/end", post(api::end_cycle))
        .route("/issues", get(api::list_issues).post(api::create_issue))
        .route(
            "/issues/{key}",
            patch(api::patch_issue)
                .get(api::get_issue)
                .delete(api::delete_issue),
        )
        .route("/issues/{key}/comments", post(api::add_comment))
        .route("/issues/{key}/activity", get(api::issue_activity))
        .route("/repos", get(api::list_repos).post(api::add_repo))
        .route("/repos/{owner}/{name}", axum::routing::delete(api::remove_repo))
        .route("/sync/reconcile", post(api::reconcile_now))
        .route("/import", post(api::import_markdown))
        .route("/notifications", get(api::list_notifications))
        .route("/notifications/read-all", post(api::mark_all_notifications_read))
        .route("/notifications/{id}/read", post(api::mark_notification_read))
        .route(
            "/notifications/prefs",
            get(api::get_notification_prefs).put(api::set_notification_prefs),
        );

    let auth_routes = Router::new()
        .route("/login", get(auth::login))
        .route("/callback", get(auth::callback))
        .route("/logout", post(auth::logout))
        .route("/dev-login", post(auth::dev_login))
        .route("/device", get(device::page).post(device::start))
        .route("/device/token", post(device::poll))
        .route("/device/approve", post(device::approve));

    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/events", get(events))
        .route("/webhooks/forgejo", post(webhook::handle))
        .nest("/api", api_routes)
        .nest("/auth", auth_routes);

    // Serve the built SPA at `/` when a web_dir is configured (production).
    // SPA routing: unknown paths fall back to index.html.
    if let Some(dir) = &state.config.web_dir {
        if dir.is_dir() {
            let index = dir.join("index.html");
            router = router.fallback_service(
                ServeDir::new(dir).not_found_service(ServeFile::new(index)),
            );
            tracing::info!(dir = %dir.display(), "serving web SPA");
        } else {
            tracing::warn!(dir = %dir.display(), "web_dir does not exist — not serving SPA");
        }
    }

    router
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let db_ok = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.db)
        .await
        .is_ok();
    let code = if db_ok {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "version": env!("CARGO_PKG_VERSION"),
            "db": db_ok,
        })),
    )
}

/// Server-sent events: every [`breadboard_core::BoardEvent`] as it is published.
async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.bus.subscribe()).filter_map(|msg| match msg {
        Ok(event) => Some(Ok(Event::default().json_data(&event).unwrap_or_default())),
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
