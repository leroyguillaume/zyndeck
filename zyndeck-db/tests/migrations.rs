//! Integration tests for the migration runner.
//!
//! These run against a real Postgres (the `postgres` service in
//! `docker-compose.yaml`); `#[sqlx::test]` provisions an isolated database per
//! test. Point `DATABASE_URL` at that server before running the suite:
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use sqlx::PgPool;
use zyndeck_db::Db;

/// `migrations = false` keeps `#[sqlx::test]` from applying migrations itself,
/// so the test exercises our own [`Db::migrate`].
#[sqlx::test(migrations = false)]
async fn migrate_enables_the_pgvector_extension(pool: PgPool) {
    let db = Db::new(pool);
    db.migrate().await.expect("migrations should apply cleanly");

    let installed: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(db.pool())
            .await
            .expect("query the extension catalog");

    assert!(installed, "the `vector` extension should be enabled");
}

#[sqlx::test(migrations = false)]
async fn migrate_is_idempotent(pool: PgPool) {
    let db = Db::new(pool);
    db.migrate().await.expect("first run should succeed");
    db.migrate()
        .await
        .expect("second run should be a no-op, not an error");
}

#[sqlx::test(migrations = false)]
async fn game_stores_localised_names(pool: PgPool) {
    let db = Db::new(pool);
    db.migrate().await.expect("migrations should apply cleanly");

    // A game references the user who created it, so insert one first.
    let creator: String = sqlx::query_scalar(
        "INSERT INTO \"user\" (username, password_hash, role) \
         VALUES ('designer', 'hash', 'user') RETURNING id::text",
    )
    .fetch_one(db.pool())
    .await
    .expect("insert a user");

    let id: String = sqlx::query_scalar(
        "INSERT INTO game (name, created_by) VALUES ($1::jsonb, $2::uuid) RETURNING id::text",
    )
    .bind(r#"{"fr": "Marvel Champions"}"#)
    .bind(&creator)
    .fetch_one(db.pool())
    .await
    .expect("insert a game with a localised name");

    let french_name: String =
        sqlx::query_scalar("SELECT name->>'fr' FROM game WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(db.pool())
            .await
            .expect("read the French name back");

    assert_eq!(french_name, "Marvel Champions");
}
