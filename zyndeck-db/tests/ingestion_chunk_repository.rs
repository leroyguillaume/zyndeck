//! Integration tests for the Postgres-backed chunk repository.
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{LanguageCode, LocalizedString, Role};
use zyndeck_db::{
    GameRepository, IngestionChunkRepository, IngestionJobRepository, NewChunk, NewGame,
    NewIngestionJob, NewUser, PgGameRepository, PgIngestionChunkRepository,
    PgIngestionJobRepository, PgUserRepository, UserRepository,
};

/// Dimension of the embedding column (`vector(1024)`), so test vectors match.
const DIM: usize = 1024;

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
            created_by: None,
        })
        .await
        .expect("create the job")
        .id
}

/// Builds a chunk at `position` with a recognisable body.
fn a_chunk(position: i32) -> NewChunk {
    NewChunk {
        position,
        heading: format!("HEADING {position}"),
        page: position + 1,
        content: format!("body {position}"),
    }
}

#[sqlx::test]
async fn replace_inserts_and_returns_chunks_in_order(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionChunkRepository::new(pool);

    let inserted = repo
        .replace(job, vec![a_chunk(0), a_chunk(1)])
        .await
        .expect("store the chunks");

    assert_eq!(inserted.len(), 2);
    assert_eq!(inserted[0].position, 0);
    assert_eq!(inserted[0].heading, "HEADING 0");
    assert_eq!(inserted[0].page, 1);
    assert_eq!(inserted[0].content, "body 0");
    assert_eq!(inserted[1].position, 1);
    // Ids are assigned by the database.
    assert_ne!(inserted[0].id, inserted[1].id);

    let found = repo.find_by_job(job).await.expect("read back");
    assert_eq!(found, inserted);
}

#[sqlx::test]
async fn replace_overwrites_previous_chunks(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionChunkRepository::new(pool);

    repo.replace(job, vec![a_chunk(0), a_chunk(1), a_chunk(2)])
        .await
        .expect("first replace");
    repo.replace(job, vec![a_chunk(0)])
        .await
        .expect("second replace");

    let found = repo.find_by_job(job).await.expect("read back");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].position, 0);
}

#[sqlx::test]
async fn find_by_job_returns_empty_when_none(pool: PgPool) {
    let repo = PgIngestionChunkRepository::new(pool);

    let found = repo.find_by_job(Uuid::new_v4()).await.expect("query");

    assert!(found.is_empty());
}

#[sqlx::test]
async fn store_embedding_is_upsertable(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionChunkRepository::new(pool.clone());
    let chunk = repo
        .replace(job, vec![a_chunk(0)])
        .await
        .expect("store the chunk")
        .remove(0);

    repo.store_embedding(chunk.id, vec![0.1; DIM])
        .await
        .expect("first store");
    repo.store_embedding(chunk.id, vec![0.2; DIM])
        .await
        .expect("overwrite");

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingestion_chunk_embedding WHERE chunk_id = $1")
            .bind(chunk.id)
            .fetch_one(&pool)
            .await
            .expect("count embeddings");
    assert_eq!(count, 1, "upsert keeps a single row per chunk");
}

#[sqlx::test]
async fn replacing_chunks_cascades_their_embeddings(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionChunkRepository::new(pool.clone());
    let chunk = repo
        .replace(job, vec![a_chunk(0)])
        .await
        .expect("store the chunk")
        .remove(0);
    repo.store_embedding(chunk.id, vec![0.1; DIM])
        .await
        .expect("store the embedding");

    // Replacing the chunks drops the old rows, and the embedding cascades away.
    repo.replace(job, vec![a_chunk(0)])
        .await
        .expect("replace the chunks");

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingestion_chunk_embedding WHERE chunk_id = $1")
            .bind(chunk.id)
            .fetch_one(&pool)
            .await
            .expect("count embeddings");
    assert_eq!(count, 0, "the orphaned embedding is gone");
}

#[sqlx::test]
async fn store_embedding_rejects_a_wrong_dimension(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionChunkRepository::new(pool);
    let chunk = repo
        .replace(job, vec![a_chunk(0)])
        .await
        .expect("store the chunk")
        .remove(0);

    // The column is `vector(1024)`; a shorter vector must be refused by Postgres.
    let result = repo.store_embedding(chunk.id, vec![0.1; DIM - 1]).await;
    assert!(result.is_err());
}
