//! Integration tests for the Postgres-backed game repository.
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
use zyndeck_core::{LanguageCode, LocalizedString, Role};
use zyndeck_db::{
    GameRepository, GameUpdate, NewGame, NewUser, PgGameRepository, PgUserRepository,
    UserRepository,
};

fn marvel_champions() -> LocalizedString {
    LocalizedString::from_pairs([("fr", "Marvel Champions"), ("en", "Marvel Champions")])
        .expect("valid language codes")
}

/// Creates a user to satisfy the `game.created_by` foreign key, returning its id.
async fn a_creator(pool: &PgPool) -> Uuid {
    PgUserRepository::new(pool.clone())
        .create(NewUser {
            username: "designer".into(),
            password_hash: "hash".into(),
            role: Role::User,
        })
        .await
        .expect("create the creator user")
        .id
}

#[sqlx::test]
async fn create_then_find_and_list(pool: PgPool) {
    let creator = a_creator(&pool).await;
    let repo = PgGameRepository::new(pool);
    let name = marvel_champions();

    let created = repo
        .create(NewGame {
            name: name.clone(),
            created_by: creator,
        })
        .await
        .expect("create the game");
    assert_eq!(created.name, name);
    assert_eq!(created.created_by, creator);

    let found = repo
        .find_by_id(created.id)
        .await
        .expect("look the game up")
        .expect("the game should exist");
    assert_eq!(found, created);

    let all = repo
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("stream games");
    assert_eq!(all, vec![created]);
}

#[sqlx::test]
async fn list_streams_every_game(pool: PgPool) {
    let creator = a_creator(&pool).await;
    let repo = PgGameRepository::new(pool);
    for _ in 0..3 {
        repo.create(NewGame {
            name: marvel_champions(),
            created_by: creator,
        })
        .await
        .expect("create a game");
    }

    let all = repo
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("stream games");
    assert_eq!(all.len(), 3);
}

#[sqlx::test]
async fn find_by_id_returns_none_when_absent(pool: PgPool) {
    let repo = PgGameRepository::new(pool);

    let missing = repo
        .find_by_id(Uuid::new_v4())
        .await
        .expect("query should succeed");

    assert!(missing.is_none());
}

#[sqlx::test]
async fn update_replaces_the_name(pool: PgPool) {
    let creator = a_creator(&pool).await;
    let repo = PgGameRepository::new(pool);
    let created = repo
        .create(NewGame {
            name: marvel_champions(),
            created_by: creator,
        })
        .await
        .expect("create the game");

    let mut renamed = LocalizedString::new();
    renamed.insert(LanguageCode::ENGLISH, "Marvel Champions: The Card Game");
    let updated = repo
        .update(
            created.id,
            GameUpdate {
                name: renamed.clone(),
            },
        )
        .await
        .expect("update the game")
        .expect("the game should exist");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, renamed);
    assert_eq!(updated.created_by, creator);

    // The change is persisted, not just echoed back.
    let reread = repo
        .find_by_id(created.id)
        .await
        .expect("query")
        .expect("the game should still exist");
    assert_eq!(reread, updated);
}

#[sqlx::test]
async fn update_returns_none_when_absent(pool: PgPool) {
    let repo = PgGameRepository::new(pool);

    let result = repo
        .update(
            Uuid::new_v4(),
            GameUpdate {
                name: marvel_champions(),
            },
        )
        .await
        .expect("query should succeed");

    assert!(result.is_none());
}

#[sqlx::test]
async fn delete_removes_the_game(pool: PgPool) {
    let creator = a_creator(&pool).await;
    let repo = PgGameRepository::new(pool);
    let created = repo
        .create(NewGame {
            name: marvel_champions(),
            created_by: creator,
        })
        .await
        .expect("create the game");

    assert!(repo.delete(created.id).await.expect("delete the game"));
    assert!(
        repo.find_by_id(created.id).await.expect("query").is_none(),
        "the game should be gone after deletion"
    );
    assert!(
        !repo.delete(created.id).await.expect("delete again"),
        "deleting a missing game returns false"
    );
}
