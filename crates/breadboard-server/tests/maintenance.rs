//! WAL checkpoint maintenance.

use std::path::Path;

use breadboard_server::db;

fn wal_path(db_path: &Path) -> std::path::PathBuf {
    let mut path = db_path.as_os_str().to_owned();
    path.push("-wal");
    std::path::PathBuf::from(path)
}

#[tokio::test]
async fn a_truncating_checkpoint_empties_the_log() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("wal.db");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let pool = db::connect(&url).await.unwrap();

    sqlx::query("CREATE TABLE t (x TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..500 {
        sqlx::query("INSERT INTO t (x) VALUES ('0123456789012345678901234567890123456789')")
            .execute(&pool)
            .await
            .unwrap();
    }
    let before = std::fs::metadata(wal_path(&path)).unwrap().len();
    assert!(before > 0, "writes grow the WAL");

    db::checkpoint(&pool, &path, false).await.unwrap();
    db::checkpoint(&pool, &path, true).await.unwrap();

    assert_eq!(
        std::fs::metadata(wal_path(&path)).unwrap().len(),
        0,
        "TRUNCATE leaves an empty WAL, which is what the daily maintenance run does"
    );
}
