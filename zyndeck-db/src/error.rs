use thiserror::Error;
use zyndeck_core::IngestionStep;

/// Errors raised by the database layer.
#[derive(Debug, Error)]
pub enum Error {
    /// Opening the connection pool failed.
    #[error("failed to connect to the database")]
    Connect(#[source] sqlx::Error),

    /// Applying migrations failed.
    #[error("failed to run database migrations")]
    Migrate(#[source] sqlx::migrate::MigrateError),

    /// A query against the database failed.
    #[error("database query failed")]
    Query(#[source] sqlx::Error),

    /// Attempted to create a user whose username already exists.
    #[error("username {0:?} is already taken")]
    UsernameTaken(String),

    /// A role value stored in the database is not a recognised role.
    #[error("invalid role {0:?} stored in the database")]
    InvalidRole(String),

    /// An ingestion step value stored in the database is not recognised.
    #[error("invalid ingestion step {0:?} stored in the database")]
    InvalidIngestionStep(String),

    /// A language value stored in the database is not a valid ISO 639-1 code.
    #[error("invalid language {0:?} stored in the database")]
    InvalidLanguage(String),

    /// A step run status value stored in the database is not recognised.
    #[error("invalid step run status {0:?} stored in the database")]
    InvalidStepRunStatus(String),

    /// Attempted to start a run for a job that already has one in progress.
    #[error("ingestion job {0} already has a run in progress")]
    JobAlreadyRunning(uuid::Uuid),

    /// Tried to start an ingestion job for a game that does not exist.
    #[error("no game with id {0}")]
    GameNotFound(uuid::Uuid),

    /// Referenced an ingestion job that does not exist.
    #[error("no ingestion job with id {0}")]
    JobNotFound(uuid::Uuid),

    /// Tried to advance or restart a job that is already completed.
    #[error("ingestion job {0} is already completed")]
    JobCompleted(uuid::Uuid),

    /// Tried to advance a job whose current step has not succeeded yet.
    #[error("ingestion job {job} cannot advance: step {step:?} has not succeeded yet")]
    StepNotSucceeded {
        job: uuid::Uuid,
        step: IngestionStep,
    },

    /// Tried to validate or restart a job that is no longer in the transcription
    /// phase (its transcript has already been validated, or the job is done).
    #[error("ingestion job {0} is no longer in the transcription phase")]
    NotInTranscriptionPhase(uuid::Uuid),

    /// Tried to continue a job still awaiting human validation of its transcript.
    #[error("ingestion job {0} must have its transcript validated before it can continue")]
    ValidationRequired(uuid::Uuid),
}

/// Convenience alias for fallible operations in this crate.
pub type Result<T> = std::result::Result<T, Error>;
