//! Integration tests for the transactional job transitions on [`Db`]
//! (`start_job` / `validate_job` / `restart_job` / `continue_job` /
//! `claim_pending_run`), which take a `FOR UPDATE` lock (or an atomic claim) so a
//! job's step/run state stays consistent and never runs in parallel.
//!
//! The pipeline runs in two phases with a human gate: `extract` produces a
//! transcript and stops; `validate_job` opens the gate (`extract → chunk`);
//! then `chunk → embed` chain straight through. Work is handed to the service as
//! a `pending` run that it claims.
//!
//! ```bash
//! DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
//!   cargo test -p zyndeck-db
//! ```

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{
    IngestionJob, IngestionStep, IngestionStepRun, LanguageCode, LocalizedString, Role,
    StepRunStatus,
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

/// Inputs for a job against `game`.
fn a_new_job(game: Uuid) -> NewIngestionJob {
    NewIngestionJob {
        game_id: game,
        source: "rules.pdf".into(),
        language: LanguageCode::ENGLISH,
        created_by: None,
    }
}

/// Creates a user + game, then starts a job for it (with its first run enqueued).
async fn start(db: &Db) -> (IngestionJob, IngestionStepRun) {
    let game = a_game(db).await;
    db.start_job(a_new_job(game)).await.expect("start the job")
}

/// Claims the job's pending run (as the service would), panicking if none.
async fn claim(db: &Db, job_id: Uuid) -> IngestionStepRun {
    db.claim_pending_run(job_id)
        .await
        .expect("claim the pending run")
        .expect("a pending run is waiting")
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
async fn start_enqueues_a_pending_first_attempt(pool: PgPool) {
    let db = Db::new(pool);

    let (job, run) = start(&db).await;

    assert_eq!(job.step, IngestionStep::Extract);
    assert_eq!(run.job_id, job.id);
    assert_eq!(run.step, IngestionStep::Extract);
    assert_eq!(run.attempt, 1);
    assert!(matches!(run.status, StepRunStatus::Pending));
}

#[sqlx::test]
async fn claim_pending_run_claims_once(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;

    // First claim flips the pending run to running.
    let run = claim(&db, job.id).await;
    assert_eq!(run.job_id, job.id);
    assert_eq!(run.step, IngestionStep::Extract);
    assert_eq!(run.attempt, 1);
    assert!(matches!(run.status, StepRunStatus::Running { .. }));

    // It is now running, so a second claim finds nothing pending.
    assert!(
        db.claim_pending_run(job.id)
            .await
            .expect("second claim succeeds")
            .is_none(),
        "a job whose run is already claimed must not be claimed again",
    );
}

#[sqlx::test]
async fn an_active_run_blocks_restart_and_continue(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;

    // The first run is pending (active), so the job is busy.
    assert!(
        matches!(db.restart_job(job.id).await, Err(Error::JobAlreadyRunning(id)) if id == job.id),
        "restart must be rejected while a run is active",
    );
    // Still on extract → continue is the wrong door; validation is required.
    assert!(
        matches!(db.continue_job(job.id).await, Err(Error::ValidationRequired(id)) if id == job.id),
        "continue must be rejected before validation",
    );
}

#[sqlx::test]
async fn restart_after_failure_enqueues_a_new_pending_attempt(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    let run = claim(&db, job.id).await;
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
    assert!(matches!(retry.status, StepRunStatus::Pending));
}

#[sqlx::test]
async fn validate_requires_a_succeeded_transcription(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    let run = claim(&db, job.id).await;
    finish(
        &db,
        run.id,
        StepOutcome::Failed {
            error: "boom".to_owned(),
        },
    )
    .await;

    assert!(
        matches!(db.validate_job(job.id).await, Err(Error::StepNotSucceeded { step, .. }) if step == IngestionStep::Extract),
        "a job whose transcription failed cannot be validated",
    );
}

#[sqlx::test]
async fn validate_opens_the_gate_and_enqueues_chunk(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    let extract = claim(&db, job.id).await;
    finish(&db, extract.id, StepOutcome::Succeeded).await;

    // Before validation the gate is shut: continue is rejected.
    assert!(
        matches!(db.continue_job(job.id).await, Err(Error::ValidationRequired(id)) if id == job.id),
    );

    let chunk = db
        .validate_job(job.id)
        .await
        .expect("validate the transcript");
    assert_eq!(chunk.step, IngestionStep::Chunk);
    assert_eq!(chunk.attempt, 1);
    assert!(matches!(chunk.status, StepRunStatus::Pending));

    let job = db
        .ingestion_jobs()
        .find_by_id(job.id)
        .await
        .expect("reload the job")
        .expect("the job exists");
    assert_eq!(job.step, IngestionStep::Chunk);
}

#[sqlx::test]
async fn phase_two_chains_chunk_then_embed_to_completed(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    let extract = claim(&db, job.id).await;
    finish(&db, extract.id, StepOutcome::Succeeded).await;

    // Validation enqueues the chunk run; the service claims and runs it.
    db.validate_job(job.id).await.expect("validate");
    let chunk = claim(&db, job.id).await;
    assert_eq!(chunk.step, IngestionStep::Chunk);
    finish(&db, chunk.id, StepOutcome::Succeeded).await;

    // From chunk onward the service chains straight through.
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
        Err(Error::NotInTranscriptionPhase(_))
    ));
}

#[sqlx::test]
async fn restart_after_validation_is_rejected(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    let extract = claim(&db, job.id).await;
    finish(&db, extract.id, StepOutcome::Succeeded).await;
    db.validate_job(job.id).await.expect("validate");

    assert!(
        matches!(db.restart_job(job.id).await, Err(Error::NotInTranscriptionPhase(id)) if id == job.id),
        "a validated job is locked into phase 2 and cannot be restarted",
    );
}

#[sqlx::test]
async fn an_aborted_transcription_can_be_restarted_but_not_validated(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    claim(&db, job.id).await;

    // Stop the running step (as the `stop` command would).
    db.step_runs()
        .abort(job.id)
        .await
        .expect("abort")
        .expect("a run was aborted");

    // Aborted is terminal-but-not-succeeded: validation is blocked, restart works.
    assert!(matches!(
        db.validate_job(job.id).await,
        Err(Error::StepNotSucceeded { .. })
    ));
    let retry = db.restart_job(job.id).await.expect("restart after abort");
    assert_eq!(retry.step, IngestionStep::Extract);
    assert_eq!(retry.attempt, 2);
    assert!(matches!(retry.status, StepRunStatus::Pending));
}

#[sqlx::test]
async fn start_for_an_unknown_game_reports_game_not_found(pool: PgPool) {
    let db = Db::new(pool);
    let unknown = Uuid::new_v4();

    let result = db.start_job(a_new_job(unknown)).await;

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
        db.validate_job(Uuid::new_v4()).await,
        Err(Error::JobNotFound(_))
    ));
    // Claiming is a lock-free UPDATE: a missing job simply has nothing pending.
    assert!(
        db.claim_pending_run(Uuid::new_v4())
            .await
            .expect("claim succeeds")
            .is_none(),
    );
}

#[sqlx::test]
async fn pending_run_job_ids_lists_jobs_with_pending_runs(pool: PgPool) {
    let db = Db::new(pool);
    // A job whose first run is still pending (just started).
    let (waiting, _run) = start(&db).await;
    // A job whose run has been claimed, so nothing is pending for it.
    let (claimed, _run) = start(&db).await;
    claim(&db, claimed.id).await;

    let ids = db.pending_run_job_ids().await.expect("list pending jobs");

    assert!(
        ids.contains(&waiting.id),
        "a job with a pending run is listed"
    );
    assert!(
        !ids.contains(&claimed.id),
        "a job whose run was claimed must not be listed",
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

#[sqlx::test]
async fn validating_a_job_notifies_the_listener(pool: PgPool) {
    let db = Db::new(pool);
    let (job, _run) = start(&db).await;
    let extract = claim(&db, job.id).await;
    finish(&db, extract.id, StepOutcome::Succeeded).await;

    // Subscribe before validating, so the validate notification is delivered.
    let mut listener = db.listen_ingestion_jobs().await.expect("subscribe");
    db.validate_job(job.id).await.expect("validate");

    let received = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .expect("a notification arrives within the timeout")
        .expect("the notification is received");
    assert_eq!(received, job.id);
}
