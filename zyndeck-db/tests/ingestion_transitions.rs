//! Integration tests for the transactional job transitions on [`Db`]
//! (`start_job` / `continue_job` / `restart_job`), which take a `FOR UPDATE`
//! lock so a job's step/run state stays consistent and never runs in parallel.
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{
    IngestionJob, IngestionMode, IngestionStep, IngestionStepRun, LanguageCode, LocalizedString,
    Role, StepRunStatus,
};
use zyndeck_db::{
    Advanced, Db, Error, GameRepository, IngestionJobRepository, IngestionStepRunRepository,
    NewGame, NewIngestionJob, NewUser, StepOutcome, UserRepository,
};

/// Creates a user and a game (to satisfy a job's foreign keys), returning the
/// game's id.
async fn a_game(db: &Db) -> Uuid {
    let user = db
        .users()
        .create(NewUser {
            // Unique per call so a test can create more than one game/job.
            username: format!("ingester-{}", Uuid::new_v4()),
            password_hash: "hash".into(),
            role: Role::User,
        })
        .await
        .expect("create the user")
        .id;
    db.games()
        .create(NewGame {
            name: LocalizedString::from_pairs([("en", "Marvel Champions")])
                .expect("valid language code"),
            created_by: user,
        })
        .await
        .expect("create the game")
        .id
}

/// Inputs for a job against `game`, in the given mode.
fn a_new_job(game: Uuid, mode: IngestionMode) -> NewIngestionJob {
    NewIngestionJob {
        game_id: game,
        source: "rules.pdf".into(),
        language: LanguageCode::ENGLISH,
        mode,
        created_by: None,
    }
}

/// Creates a user + game, then starts a job for it (with its first run begun).
async fn start(db: &Db) -> (IngestionJob, IngestionStepRun) {
    let game = a_game(db).await;
    db.start_job(a_new_job(game, IngestionMode::Manual))
        .await
        .expect("start the job")
}

/// Finishes a run with the given outcome, panicking on any error.
async fn finish(db: &Db, run_id: Uuid, outcome: StepOutcome) {
    db.step_runs()
        .finish(run_id, outcome)
        .await
        .expect("finish the run")
        .expect("the run should exist");
}

#[sqlx::test]
async fn start_creates_a_job_with_a_running_first_attempt(pool: PgPool) {
    let db = Db::new(pool);

    let (job, run) = start(&db).await;

    assert_eq!(job.step, IngestionStep::Extract);
    assert_eq!(run.job_id, job.id);
    assert_eq!(run.step, IngestionStep::Extract);
    assert_eq!(run.attempt, 1);
    assert!(matches!(run.status, StepRunStatus::Running { .. }));
}

#[sqlx::test]
async fn an_active_run_blocks_restart_and_continue(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;

    // The first run is still active, so the job is busy.
    assert!(
        matches!(db.restart_job(job.id).await, Err(Error::JobAlreadyRunning(id)) if id == job.id),
        "restart must be rejected while a run is active",
    );
    assert!(
        matches!(db.continue_job(job.id).await, Err(Error::StepNotSucceeded { job: id, .. }) if id == job.id),
        "continue must be rejected while the step has not succeeded",
    );
}

#[sqlx::test]
async fn restart_after_failure_increments_the_attempt(pool: PgPool) {
    let db = Db::new(pool);
    let (job, run) = start(&db).await;
    finish(
        &db,
        run.id,
        StepOutcome::Failed {
            error: "boom".to_owned(),
        },
    )
    .await;

    let retry = db.restart_job(job.id).await.expect("restart the step");

    assert_eq!(retry.step, IngestionStep::Extract);
    assert_eq!(retry.attempt, 2);
    assert!(matches!(retry.status, StepRunStatus::Running { .. }));
}

#[sqlx::test]
async fn continue_requires_the_current_step_to_have_succeeded(pool: PgPool) {
    let db = Db::new(pool);
    let (job, run) = start(&db).await;
    finish(
        &db,
        run.id,
        StepOutcome::Failed {
            error: "boom".to_owned(),
        },
    )
    .await;

    assert!(
        matches!(db.continue_job(job.id).await, Err(Error::StepNotSucceeded { step, .. }) if step == IngestionStep::Extract),
    );
}

#[sqlx::test]
async fn continue_advances_step_by_step_to_completed(pool: PgPool) {
    let db = Db::new(pool);
    let (job, extract) = start(&db).await;

    finish(&db, extract.id, StepOutcome::Succeeded).await;
    let chunk = match db.continue_job(job.id).await.expect("advance to chunk") {
        Advanced::Running(run) => run,
        Advanced::Completed => panic!("expected chunk, got completed"),
    };
    assert_eq!(chunk.step, IngestionStep::Chunk);
    assert_eq!(chunk.attempt, 1);

    finish(&db, chunk.id, StepOutcome::Succeeded).await;
    let embed = match db.continue_job(job.id).await.expect("advance to embed") {
        Advanced::Running(run) => run,
        Advanced::Completed => panic!("expected embed, got completed"),
    };
    assert_eq!(embed.step, IngestionStep::Embed);

    finish(&db, embed.id, StepOutcome::Succeeded).await;
    assert_eq!(
        db.continue_job(job.id).await.expect("advance past embed"),
        Advanced::Completed,
    );

    // Once completed, there is nothing left to advance or restart.
    assert!(matches!(
        db.continue_job(job.id).await,
        Err(Error::JobCompleted(_))
    ));
    assert!(matches!(
        db.restart_job(job.id).await,
        Err(Error::JobCompleted(_))
    ));
}

#[sqlx::test]
async fn an_aborted_step_can_be_restarted_but_not_continued(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;

    // Stop the running step (as the `stop` command would).
    db.step_runs()
        .abort(job.id)
        .await
        .expect("abort")
        .expect("a run was aborted");

    // Aborted is terminal-but-not-succeeded: continue is blocked, restart works.
    assert!(matches!(
        db.continue_job(job.id).await,
        Err(Error::StepNotSucceeded { .. })
    ));
    let retry = db.restart_job(job.id).await.expect("restart after abort");
    assert_eq!(retry.step, IngestionStep::Extract);
    assert_eq!(retry.attempt, 2);
}

#[sqlx::test]
async fn start_for_an_unknown_game_reports_game_not_found(pool: PgPool) {
    let db = Db::new(pool);
    let unknown = Uuid::new_v4();

    let result = db
        .start_job(a_new_job(unknown, IngestionMode::Manual))
        .await;

    assert!(
        matches!(result, Err(Error::GameNotFound(id)) if id == unknown),
        "expected GameNotFound, got {result:?}",
    );
}

#[sqlx::test]
async fn transitions_on_a_missing_job_report_not_found(pool: PgPool) {
    let db = Db::new(pool);

    assert!(matches!(
        db.continue_job(Uuid::new_v4()).await,
        Err(Error::JobNotFound(_))
    ));
    assert!(matches!(
        db.restart_job(Uuid::new_v4()).await,
        Err(Error::JobNotFound(_))
    ));
    assert!(matches!(
        db.begin_initial_run(Uuid::new_v4()).await,
        Err(Error::JobNotFound(_))
    ));
}

#[sqlx::test]
async fn begin_initial_run_claims_a_fresh_job_once(pool: PgPool) {
    let db = Db::new(pool);
    let game = a_game(&db).await;
    let job = db
        .ingestion_jobs()
        .create(a_new_job(game, IngestionMode::Auto))
        .await
        .expect("create the job");

    // First claim begins the first step's run.
    let run = db
        .begin_initial_run(job.id)
        .await
        .expect("claim the job")
        .expect("a fresh job is claimable");
    assert_eq!(run.job_id, job.id);
    assert_eq!(run.step, IngestionStep::Extract);
    assert_eq!(run.attempt, 1);
    assert!(matches!(run.status, StepRunStatus::Running { .. }));

    // It is now in flight, so a second claim finds nothing to do.
    assert!(
        db.begin_initial_run(job.id)
            .await
            .expect("second claim succeeds")
            .is_none(),
        "a job that already has a run must not be claimed again",
    );
}

#[sqlx::test]
async fn begin_initial_run_skips_a_job_that_has_already_run(pool: PgPool) {
    let db = Db::new(pool);
    // `start` begins a run, so the job is no longer fresh.
    let (job, _run) = start(&db).await;

    assert!(
        db.begin_initial_run(job.id)
            .await
            .expect("claim succeeds")
            .is_none(),
    );
}

#[sqlx::test]
async fn unprocessed_job_ids_lists_only_fresh_jobs(pool: PgPool) {
    let db = Db::new(pool);
    let game = a_game(&db).await;
    let fresh = db
        .ingestion_jobs()
        .create(a_new_job(game, IngestionMode::Auto))
        .await
        .expect("create the fresh job")
        .id;
    // A second job that has already been started (it has a run).
    let (started, _run) = start(&db).await;

    let ids = db
        .unprocessed_job_ids()
        .await
        .expect("list unprocessed jobs");

    assert!(ids.contains(&fresh), "a fresh job must be listed");
    assert!(
        !ids.contains(&started.id),
        "a job that already has a run must not be listed",
    );
}

#[sqlx::test]
async fn creating_a_job_notifies_the_listener(pool: PgPool) {
    let db = Db::new(pool);
    // Subscribe before the job is created, so the notification is delivered.
    let mut listener = db.listen_ingestion_jobs().await.expect("subscribe");

    let (job, _run) = start(&db).await;

    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .expect("a notification arrives within the timeout")
        .expect("the notification is received");
    assert_eq!(received, job.id);
}
