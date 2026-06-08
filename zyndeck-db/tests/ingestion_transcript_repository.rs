//! Integration tests for the Postgres-backed transcript repository.
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{IngestionMode, LanguageCode, LocalizedString, Role};
use zyndeck_db::{
    GameRepository, IngestionJobRepository, IngestionTranscriptRepository, NewGame,
    NewIngestionJob, NewUser, PgGameRepository, PgIngestionJobRepository,
    PgIngestionTranscriptRepository, PgUserRepository, UserRepository,
};

/// Creates a job (with the user + game its foreign keys need), returning its id.
async fn a_job(pool: &PgPool) -> Uuid {
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
    PgIngestionJobRepository::new(pool.clone())
        .create(NewIngestionJob {
            game_id: game,
            source: "rules.pdf".into(),
            language: LanguageCode::ENGLISH,
            mode: IngestionMode::Manual,
            created_by: None,
        })
        .await
        .expect("create the job")
        .id
}

#[sqlx::test]
async fn upsert_then_find_round_trips(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionTranscriptRepository::new(pool);

    repo.upsert(job, "## HEADING\n\nbody".to_owned())
        .await
        .expect("store the transcript");

    let found = repo.find(job).await.expect("query").expect("it exists");
    assert_eq!(found, "## HEADING\n\nbody");
}

#[sqlx::test]
async fn upsert_replaces_an_existing_transcript(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionTranscriptRepository::new(pool);

    repo.upsert(job, "first".to_owned())
        .await
        .expect("first upsert");
    repo.upsert(job, "second".to_owned())
        .await
        .expect("second upsert");

    let found = repo.find(job).await.expect("query").expect("it exists");
    assert_eq!(found, "second");
}

#[sqlx::test]
async fn find_returns_none_when_absent(pool: PgPool) {
    let repo = PgIngestionTranscriptRepository::new(pool);

    let missing = repo.find(Uuid::new_v4()).await.expect("query");

    assert!(missing.is_none());
}
