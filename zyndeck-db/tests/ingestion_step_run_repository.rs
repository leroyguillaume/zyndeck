//! Integration tests for the Postgres-backed step-run repository.
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
use zyndeck_core::{IngestionStep, LanguageCode, LocalizedString, Role, StepRunStatus};
use zyndeck_db::{
    Error, GameRepository, IngestionJobRepository, IngestionStepRunRepository, NewGame,
    NewIngestionJob, NewUser, PgGameRepository, PgIngestionJobRepository,
    PgIngestionStepRunRepository, PgUserRepository, StepOutcome, UserRepository,
};

/// Creates an ingestion job (with the user + game its foreign keys need),
/// returning its id.
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

#[sqlx::test]
async fn begin_starts_a_running_first_attempt(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);

    let run = repo
        .begin(job, IngestionStep::Extract)
        .await
        .expect("begin the run");

    assert_eq!(run.job_id, job);
    assert_eq!(run.step, IngestionStep::Extract);
    assert_eq!(run.attempt, 1);
    assert!(matches!(run.status, StepRunStatus::Running { .. }));
    assert!(run.status.started_at().is_some());
    assert!(run.status.completed_at().is_none());
    assert!(run.status.error().is_none());
}

#[sqlx::test]
async fn begin_rejects_a_second_active_run_for_the_same_job(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);

    repo.begin(job, IngestionStep::Extract)
        .await
        .expect("first run begins");

    let conflict = repo.begin(job, IngestionStep::Extract).await;
    assert!(
        matches!(conflict, Err(Error::JobAlreadyRunning(id)) if id == job),
        "a second active run must be rejected, got {conflict:?}",
    );
}

#[sqlx::test]
async fn finish_records_failure_then_a_retry_increments_the_attempt(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);

    let first = repo
        .begin(job, IngestionStep::Extract)
        .await
        .expect("first attempt begins");
    let failed = repo
        .finish(
            first.id,
            StepOutcome::Failed {
                error: "boom".to_owned(),
            },
        )
        .await
        .expect("finish the run")
        .expect("the run should exist");
    assert!(matches!(failed.status, StepRunStatus::Failed { .. }));
    assert!(failed.status.completed_at().is_some());
    assert_eq!(failed.status.error(), Some("boom"));

    // Once the first attempt is no longer active, a retry is allowed and counts up.
    let second = repo
        .begin(job, IngestionStep::Extract)
        .await
        .expect("retry begins");
    assert_eq!(second.attempt, 2);

    let latest = repo
        .find_latest(job)
        .await
        .expect("query")
        .expect("there is a run");
    assert_eq!(latest.id, second.id);
    assert_eq!(latest.attempt, 2);
}

#[sqlx::test]
async fn finish_succeeded_clears_the_error(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);
    let run = repo
        .begin(job, IngestionStep::Extract)
        .await
        .expect("begin");

    let done = repo
        .finish(run.id, StepOutcome::Succeeded)
        .await
        .expect("finish")
        .expect("the run should exist");

    assert!(matches!(done.status, StepRunStatus::Succeeded { .. }));
    assert!(done.status.completed_at().is_some());
    assert!(done.status.error().is_none());
}

#[sqlx::test]
async fn find_latest_returns_none_when_absent(pool: PgPool) {
    let repo = PgIngestionStepRunRepository::new(pool);

    let missing = repo
        .find_latest(Uuid::new_v4())
        .await
        .expect("query should succeed");

    assert!(missing.is_none());
}

#[sqlx::test]
async fn abort_stops_the_active_run(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);
    let run = repo
        .begin(job, IngestionStep::Extract)
        .await
        .expect("begin");

    let aborted = repo
        .abort(job)
        .await
        .expect("abort")
        .expect("a run was aborted");
    assert_eq!(aborted.id, run.id);
    assert!(matches!(aborted.status, StepRunStatus::Aborted { .. }));
    assert!(aborted.status.completed_at().is_some());

    let found = repo
        .find(run.id)
        .await
        .expect("query")
        .expect("the run should exist");
    assert!(matches!(found.status, StepRunStatus::Aborted { .. }));
}

#[sqlx::test]
async fn abort_returns_none_when_nothing_is_running(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);

    assert!(repo.abort(job).await.expect("abort").is_none());
}

#[sqlx::test]
async fn finishing_an_aborted_run_does_not_overwrite_it(pool: PgPool) {
    let job = a_job(&pool).await;
    let repo = PgIngestionStepRunRepository::new(pool);
    let run = repo
        .begin(job, IngestionStep::Extract)
        .await
        .expect("begin");
    repo.abort(job).await.expect("abort").expect("aborted");

    let finished = repo
        .finish(run.id, StepOutcome::Succeeded)
        .await
        .expect("finish");
    assert!(
        finished.is_none(),
        "finish must not overwrite an aborted run",
    );

    let found = repo
        .find(run.id)
        .await
        .expect("query")
        .expect("the run should exist");
    assert!(matches!(found.status, StepRunStatus::Aborted { .. }));
}
