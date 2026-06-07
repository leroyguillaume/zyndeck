//! Integration tests for the Postgres-backed user repository.
//!
//! `#[sqlx::test]` applies the migrations and hands over an isolated database
//! per test. Point `DATABASE_URL` at the compose Postgres before running:
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use futures::TryStreamExt;
use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::Role;
use zyndeck_db::{Error, NewUser, PgUserRepository, UserRepository};

// The repository stores the hash verbatim; tests use any non-null string.
const HASH: &str = "argon2-hash";

#[sqlx::test]
async fn create_then_find(pool: PgPool) {
    let repo = PgUserRepository::new(pool);

    let created = repo
        .create(NewUser {
            username: "alice".into(),
            password_hash: HASH.into(),
            role: Role::Admin,
        })
        .await
        .expect("create the user");
    assert_eq!(created.username, "alice");
    assert_eq!(created.role, Role::Admin);

    let by_id = repo
        .find_by_id(created.id)
        .await
        .expect("look up by id")
        .expect("the user should exist");
    assert_eq!(by_id, created);

    let by_username = repo
        .find_by_username("alice".into())
        .await
        .expect("look up by username")
        .expect("the user should exist");
    assert_eq!(by_username, created);
}

#[sqlx::test]
async fn username_must_be_unique(pool: PgPool) {
    let repo = PgUserRepository::new(pool);
    repo.create(NewUser {
        username: "bob".into(),
        password_hash: HASH.into(),
        role: Role::User,
    })
    .await
    .expect("first create succeeds");

    let err = repo
        .create(NewUser {
            username: "bob".into(),
            password_hash: HASH.into(),
            role: Role::SuperAdmin,
        })
        .await
        .expect_err("a duplicate username must fail");

    match err {
        Error::UsernameTaken(name) => assert_eq!(name, "bob"),
        other => panic!("expected UsernameTaken, got {other:?}"),
    }
}

#[sqlx::test]
async fn find_credentials_returns_id_and_hash(pool: PgPool) {
    let repo = PgUserRepository::new(pool);
    let user = repo
        .create(NewUser {
            username: "dave".into(),
            password_hash: HASH.into(),
            role: Role::User,
        })
        .await
        .expect("create the user");

    let credentials = repo
        .find_credentials_by_username("dave".into())
        .await
        .expect("query")
        .expect("credentials should exist");
    assert_eq!(credentials.id, user.id);
    assert_eq!(credentials.password_hash, HASH);

    assert!(
        repo.find_credentials_by_username("ghost".into())
            .await
            .expect("query")
            .is_none()
    );
}

#[sqlx::test]
async fn find_returns_none_when_absent(pool: PgPool) {
    let repo = PgUserRepository::new(pool);

    assert!(
        repo.find_by_id(Uuid::new_v4())
            .await
            .expect("query")
            .is_none()
    );
    assert!(
        repo.find_by_username("nobody".into())
            .await
            .expect("query")
            .is_none()
    );
}

#[sqlx::test]
async fn list_and_delete(pool: PgPool) {
    let repo = PgUserRepository::new(pool);
    let user = repo
        .create(NewUser {
            username: "carol".into(),
            password_hash: HASH.into(),
            role: Role::User,
        })
        .await
        .expect("create the user");

    let users = repo
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("stream users");
    assert_eq!(users, vec![user.clone()]);

    assert!(repo.delete(user.id).await.expect("delete the user"));
    let remaining = repo
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("stream users");
    assert!(
        remaining.is_empty(),
        "the user should be gone after deletion"
    );
    assert!(
        !repo.delete(user.id).await.expect("delete again"),
        "deleting a missing user returns false"
    );
}

#[sqlx::test]
async fn upsert_inserts_then_resets(pool: PgPool) {
    let repo = PgUserRepository::new(pool);

    // First upsert inserts.
    let created = repo
        .upsert_by_username(NewUser {
            username: "admin".into(),
            password_hash: HASH.into(),
            role: Role::SuperAdmin,
        })
        .await
        .expect("insert via upsert");
    assert_eq!(created.role, Role::SuperAdmin);

    // Second upsert on the same username updates in place (same id, new role).
    let updated = repo
        .upsert_by_username(NewUser {
            username: "admin".into(),
            password_hash: "new-hash".into(),
            role: Role::Admin,
        })
        .await
        .expect("update via upsert");
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.role, Role::Admin);

    // Still a single row.
    let all = repo
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("stream users");
    assert_eq!(all.len(), 1);
}
