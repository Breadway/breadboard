//! HTTP surface: health, SSE, auth, and the `/api` REST routes.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use futures::stream::Stream;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::{api, artifacts, auth, device, webhook, AppState};

pub fn router(state: AppState) -> Router {
    let api_routes = Router::new()
        .route("/me", get(auth::me))
        .route(
            "/projects",
            get(api::list_projects).post(api::create_project),
        )
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
        .route("/cycles/{id}", axum::routing::patch(api::update_cycle))
        .route("/cycles/{id}/start", post(api::start_cycle))
        .route("/cycles/{id}/end", post(api::end_cycle))
        .route("/issues", get(api::list_issues).post(api::create_issue))
        .route(
            "/issues/{key}",
            patch(api::patch_issue)
                .get(api::get_issue)
                .delete(api::delete_issue),
        )
        .route("/issues/{key}/project", put(api::move_issue_project))
        .route("/issues/{key}/comments", post(api::add_comment))
        .route("/issues/{key}/activity", get(api::issue_activity))
        // --- issue artifacts (BB-160) ------------------------------------
        .route(
            "/issues/{key}/artifacts",
            get(artifacts::list).post(artifacts::upload).layer(
                axum::extract::DefaultBodyLimit::max(artifacts::MAX_BODY_BYTES),
            ),
        )
        .route(
            "/issues/{key}/artifacts/base64",
            post(artifacts::upload_base64).layer(axum::extract::DefaultBodyLimit::max(
                artifacts::MAX_BASE64_BODY_BYTES,
            )),
        )
        .route("/artifacts/{id}", get(artifacts::download))
        // -----------------------------------------------------------------
        .route("/repos", get(api::list_repos).post(api::add_repo))
        .route(
            "/repos/{owner}/{name}",
            axum::routing::delete(api::remove_repo),
        )
        .route("/agents", get(api::list_agents))
        .route("/agents/register", post(api::register_agent))
        .route("/agents/roster", get(api::agent_roster))
        .route("/agents/message", put(api::set_agent_message))
        .route("/agents/{name}/kill", post(api::kill_agent))
        .route("/agents/{name}/pause", post(api::pause_agent))
        .route("/agents/{name}/resume", post(api::resume_agent))
        .route("/agents/{name}/hide", post(api::hide_agent))
        .route("/agents/{name}/unhide", post(api::unhide_agent))
        .route("/agents/{name}/rename", post(api::rename_agent))
        .route(
            "/chat/rooms",
            get(api::list_chat_rooms).post(api::create_chat_room),
        )
        .route(
            "/chat/rooms/{id}",
            axum::routing::delete(api::delete_chat_room),
        )
        .route("/chat/mentions", get(api::list_chat_mentions))
        .route("/chat/rooms/{id}/messages", get(api::chat_messages))
        .route("/chat/messages", post(api::post_chat_message))
        .route(
            "/chat/messages/{id}",
            axum::routing::delete(api::delete_chat_message),
        )
        .route("/chat/rooms/{id}/compact", post(api::compact_chat_room))
        .route(
            "/chat/uploads",
            post(api::upload_chat_file)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/chat/uploads/base64",
            post(api::upload_chat_file_base64)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/chat/uploads/{id}", get(api::get_chat_upload))
        .route("/chat/dm", post(api::open_dm))
        .route("/chat/pushes", get(api::list_chat_pushes))
        .route("/chat/pushes/ack", post(api::ack_chat_pushes))
        .route(
            "/chat/push-subs",
            get(api::get_chat_push_subs).put(api::set_chat_push_sub),
        )
        .route("/agents/status", get(api::agent_status))
        .route(
            "/issues/{key}/claim",
            post(api::claim_issue)
                .put(api::heartbeat_claim)
                .delete(api::release_claim),
        )
        .route(
            "/claims/{id}",
            axum::routing::delete(api::release_claim_by_id),
        )
        .route("/sync/reconcile", post(api::reconcile_now))
        .route("/import", post(api::import_markdown))
        .route("/notifications", get(api::list_notifications))
        .route(
            "/notifications/read-all",
            post(api::mark_all_notifications_read),
        )
        .route(
            "/notifications/{id}/read",
            post(api::mark_notification_read),
        )
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
            router = router
                .fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)));
            tracing::info!(dir = %dir.display(), "serving web SPA");
        } else {
            tracing::warn!(dir = %dir.display(), "web_dir does not exist: not serving SPA");
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
            // Public so the SPA can decide whether to offer the dev-login
            // form (local demos); it 403s unless BREADBOARD_DEV_LOGIN=1.
            "dev_login": crate::auth::dev_login_enabled(),
        })),
    )
}

/// Server-sent events: every [`breadboard_core::BoardEvent`] as it is published.
async fn events(
    State(state): State<AppState>,
    _user: auth::AuthUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut shutdown = state.shutdown.subscribe();
    let shutdown = async move {
        if !*shutdown.borrow() {
            let _ = shutdown.changed().await;
        }
    };
    let stream = futures::StreamExt::take_until(event_stream(state.bus.subscribe()), shutdown);

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn event_stream(
    receiver: tokio::sync::broadcast::Receiver<breadboard_core::BoardEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    BroadcastStream::new(receiver)
        // A lagged receiver has missed board changes. End the response so
        // EventSource reconnects and the client can fetch a fresh snapshot.
        .take_while(|msg| msg.is_ok())
        .map(|msg| match msg {
            Ok(event) => Ok(Event::default().json_data(&event).unwrap_or_default()),
            Err(_) => unreachable!("broadcast errors end the event stream"),
        })
}

#[cfg(test)]
mod tests {
    use super::event_stream;
    use breadboard_core::BoardEvent;
    use futures::StreamExt;

    #[tokio::test]
    async fn broadcast_lag_ends_event_stream_for_resync() {
        let (tx, receiver) = tokio::sync::broadcast::channel(1);
        tx.send(BoardEvent::CardCreated {
            id: 1,
            key: "BREAD-1".into(),
        })
        .unwrap();
        tx.send(BoardEvent::CardCreated {
            id: 2,
            key: "BREAD-2".into(),
        })
        .unwrap();

        let mut stream = Box::pin(event_stream(receiver));
        assert!(stream.next().await.is_none());
    }
}
