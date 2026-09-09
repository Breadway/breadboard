//! Daily notification digest: window selection and the empty-set skip.

mod common;

use breadboard_core::NotificationKind;
use breadboard_server::store::{self, NotificationPrefs};
use breadboard_server::notify;

fn prefs(digest: bool) -> NotificationPrefs {
    NotificationPrefs {
        assigned: true,
        mention: true,
        cycle: true,
        digest,
    }
}

async fn last_digest_at(db: &sqlx::SqlitePool, user_id: i64) -> Option<String> {
    store::digest_recipients(db)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.user_id == user_id)
        .and_then(|r| r.last_digest_at)
}

fn digest_bodies(ns: &[breadboard_core::Notification]) -> Vec<String> {
    ns.iter()
        .filter(|n| n.kind == NotificationKind::Digest)
        .map(|n| n.body.clone())
        .collect()
}

#[tokio::test]
async fn digest_window_selection_and_empty_skip() {
    let app = common::spawn().await;
    let db = &app.db;

    // user 1 is created by the harness; add user 2.
    let u1 = 1_i64;
    let u2: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (2, 'other', 'Other') RETURNING id",
    )
    .fetch_one(db)
    .await
    .unwrap();

    store::set_prefs(db, u1, prefs(true)).await.unwrap();
    store::set_prefs(db, u2, prefs(false)).await.unwrap();

    // 1. Nothing pending: no digest, and no last-sent time recorded.
    notify::send_daily_digest(&app.state).await;
    assert!(store::list_notifications(db, u1, false).await.unwrap().is_empty());
    assert!(last_digest_at(db, u1).await.is_none());

    // 2. Three unread items for u1, one for the opted-out u2.
    for i in 0..3 {
        store::create_notification(db, u1, NotificationKind::Assigned, None, &format!("item {i}"))
            .await
            .unwrap();
    }
    store::create_notification(db, u2, NotificationKind::Assigned, None, "for other")
        .await
        .unwrap();

    notify::send_daily_digest(&app.state).await;

    let n1 = store::list_notifications(db, u1, false).await.unwrap();
    let bodies = digest_bodies(&n1);
    assert_eq!(bodies.len(), 1, "one digest for u1");
    assert!(bodies[0].contains("3 unread updates"), "body: {}", bodies[0]);
    assert!(bodies[0].contains("item 0") && bodies[0].contains("item 2"));

    let n2 = store::list_notifications(db, u2, false).await.unwrap();
    assert!(digest_bodies(&n2).is_empty(), "u2 opted out, no digest");
    assert!(last_digest_at(db, u1).await.is_some());

    // 3. Immediate re-run: the three originals are before the window and the
    //    digest row is excluded from its own window, so nothing new is sent.
    notify::send_daily_digest(&app.state).await;
    let n1 = store::list_notifications(db, u1, false).await.unwrap();
    assert_eq!(digest_bodies(&n1).len(), 1, "no second digest without new items");

    // 4. One new unread item: the next digest covers only that one.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    store::create_notification(db, u1, NotificationKind::Mention, None, "fresh mention")
        .await
        .unwrap();
    notify::send_daily_digest(&app.state).await;

    let n1 = store::list_notifications(db, u1, false).await.unwrap();
    let bodies = digest_bodies(&n1);
    assert_eq!(bodies.len(), 2, "second digest sent for the new item");
    // list_notifications orders newest first.
    assert!(bodies[0].contains("1 unread update\n"), "body: {}", bodies[0]);
    assert!(bodies[0].contains("fresh mention"));
}

#[tokio::test]
async fn digest_skips_items_read_before_it_runs() {
    let app = common::spawn().await;
    let db = &app.db;
    let u1 = 1_i64;
    store::set_prefs(db, u1, prefs(true)).await.unwrap();

    let a = store::create_notification(db, u1, NotificationKind::Assigned, None, "still unread")
        .await
        .unwrap();
    let b = store::create_notification(db, u1, NotificationKind::Mention, None, "already handled")
        .await
        .unwrap();
    store::mark_notification_read(db, u1, b).await.unwrap();

    notify::send_daily_digest(&app.state).await;

    let bodies = digest_bodies(&store::list_notifications(db, u1, false).await.unwrap());
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("1 unread update\n"));
    assert!(bodies[0].contains("still unread"));
    assert!(!bodies[0].contains("already handled"));
    let _ = a;
}
