//! Integration tests for the Postgres-backed ingestion-job repository.
//!
//! `#[sqlx::test]` applies the migrations and hands over an isolated database
//! per test. Point `DATABASE_URL` at the compose Postgres before running:
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{IngestionStep, LanguageCode, LocalizedString, Role};
use zyndeck_db::{
    Error, GameRepository, IngestionJobRepository, NewGame, NewIngestionJob, NewUser,
    PgGameRepository, PgIngestionJobRepository, PgUserRepository, UserRepository,
};

/// Creates a user and a game (to satisfy the job's foreign keys), returning
/// their ids.
async fn a_user_and_game(pool: &PgPool) -> (Uuid, Uuid) {
    let user = PgUserRepository::new(pool.clone())
        .create(NewUser {
            username: "ingester".into(),
            password_hash: "hash".into(),
            role: Role::User,
        })
        .await
        .expect("create the user")
        .id;
    let game = PgGameRepository::new(pool.clone())
        .create(NewGame {
            name: LocalizedString::from_pairs([("en", "Marvel Champions")])
                .expect("valid language code"),
            created_by: user,
        })
        .await
        .expect("create the game")
        .id;
    (user, game)
}

/// Builds the inputs for a job against `game_id`.
fn a_new_job(game_id: Uuid, created_by: Option<Uuid>) -> NewIngestionJob {
    NewIngestionJob {
        game_id,
        source: "rules.pdf".into(),
        language: LanguageCode::ENGLISH,
        created_by,
    }
}

#[sqlx::test]
async fn create_starts_on_the_first_step_and_can_be_found(pool: PgPool) {
    let (user, game) = a_user_and_game(&pool).await;
    let repo = PgIngestionJobRepository::new(pool);

    let created = repo
        .create(a_new_job(game, Some(user)))
        .await
        .expect("create the job");
    assert_eq!(created.step, IngestionStep::FIRST);
    assert_eq!(created.created_by, Some(user));
    assert_eq!(created.game_id, game);
    assert_eq!(created.source, std::path::Path::new("rules.pdf"));
    assert_eq!(created.language, LanguageCode::ENGLISH);

    let found = repo
        .find_by_id(created.id)
        .await
        .expect("look the job up")
        .expect("the job should exist");
    assert_eq!(found, created);
}

#[sqlx::test]
async fn create_allows_an_anonymous_job(pool: PgPool) {
    let (_, game) = a_user_and_game(&pool).await;
    let repo = PgIngestionJobRepository::new(pool);

    let created = repo
        .create(a_new_job(game, None))
        .await
        .expect("create the job");

    assert_eq!(created.created_by, None);
}

#[sqlx::test]
async fn update_step_persists_the_new_step(pool: PgPool) {
    let (_, game) = a_user_and_game(&pool).await;
    let repo = PgIngestionJobRepository::new(pool);
    let created = repo
        .create(a_new_job(game, None))
        .await
        .expect("create the job");

    let updated = repo
        .update_step(created.id, IngestionStep::Chunk)
        .await
        .expect("update the step")
        .expect("the job should exist");
    assert_eq!(updated.step, IngestionStep::Chunk);
    assert_eq!(updated.id, created.id);

    // The change is persisted, not just echoed back.
    let reread = repo
        .find_by_id(created.id)
        .await
        .expect("query")
        .expect("the job should still exist");
    assert_eq!(reread.step, IngestionStep::Chunk);
}

#[sqlx::test]
async fn create_for_an_unknown_game_reports_game_not_found(pool: PgPool) {
    let repo = PgIngestionJobRepository::new(pool);
    let unknown = Uuid::new_v4();

    let result = repo.create(a_new_job(unknown, None)).await;

    assert!(
        matches!(result, Err(Error::GameNotFound(id)) if id == unknown),
        "expected GameNotFound, got {result:?}",
    );
}

#[sqlx::test]
async fn find_by_id_returns_none_when_absent(pool: PgPool) {
    let repo = PgIngestionJobRepository::new(pool);

    let missing = repo
        .find_by_id(Uuid::new_v4())
        .await
        .expect("query should succeed");

    assert!(missing.is_none());
}

#[sqlx::test]
async fn update_step_returns_none_when_absent(pool: PgPool) {
    let repo = PgIngestionJobRepository::new(pool);

    let result = repo
        .update_step(Uuid::new_v4(), IngestionStep::Embed)
        .await
        .expect("query should succeed");

    assert!(result.is_none());
}
