use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use crate::LanguageCode;

/// A single run of the rule-ingestion pipeline for one document.
///
/// A pure domain entity (like [`crate::Game`], deliberately **not**
/// `Serialize`/`Deserialize`): the wire and storage formats are the boundary
/// layers' concern. The job advances one [`IngestionStep`] at a time so the
/// pipeline can hand control back between steps — e.g. for a human to review and
/// fix the extracted transcript before it is chunked and embedded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestionJob {
    /// Stable identifier, assigned by the database on creation.
    pub id: Uuid,
    /// The game whose rules are being ingested.
    pub game_id: Uuid,
    /// Path to the source document to ingest. Kept so the extract step can be
    /// re-run (via `restart`) without re-supplying it.
    pub source: PathBuf,
    /// Language of the source document.
    pub language: LanguageCode,
    /// The step the job is currently on; [`IngestionStep::Completed`] once the
    /// pipeline is done. The outcome of each attempt lives in the job's
    /// [`IngestionStepRun`] history, not here.
    pub step: IngestionStep,
    /// Id of the user who started the job, if any — CLI runs may be anonymous.
    pub created_by: Option<Uuid>,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
}

/// A stage of the ingestion pipeline.
///
/// A job works through these in order, one per invocation, pausing between them
/// so the output of each can be reviewed (and, for
/// [`Extract`](IngestionStep::Extract), corrected) before moving on. `step`
/// records the step the job is currently on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IngestionStep {
    /// Extract text from the source document into a reviewable transcript.
    Extract,
    /// Split the (reviewed) transcript into chunks.
    Chunk,
    /// Embed the chunks and store them for retrieval.
    Embed,
    /// Nothing left to do.
    Completed,
}

impl IngestionStep {
    /// The step a freshly created job starts on.
    pub const FIRST: IngestionStep = IngestionStep::Extract;

    /// The step that follows this one. [`Completed`](IngestionStep::Completed) is
    /// terminal and follows itself.
    pub fn next(self) -> IngestionStep {
        match self {
            IngestionStep::Extract => IngestionStep::Chunk,
            IngestionStep::Chunk => IngestionStep::Embed,
            IngestionStep::Embed => IngestionStep::Completed,
            IngestionStep::Completed => IngestionStep::Completed,
        }
    }

    /// Whether the pipeline has finished and there is nothing left to run.
    pub fn is_completed(self) -> bool {
        matches!(self, IngestionStep::Completed)
    }

    /// The storage/representation string.
    pub fn as_str(self) -> &'static str {
        match self {
            IngestionStep::Extract => "extract",
            IngestionStep::Chunk => "chunk",
            IngestionStep::Embed => "embed",
            IngestionStep::Completed => "completed",
        }
    }
}

/// Returned when a string is not a recognised [`IngestionStep`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid ingestion step {0:?}: expected one of extract, chunk, embed, completed")]
pub struct ParseIngestionStepError(pub String);

impl FromStr for IngestionStep {
    type Err = ParseIngestionStepError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "extract" => Ok(IngestionStep::Extract),
            "chunk" => Ok(IngestionStep::Chunk),
            "embed" => Ok(IngestionStep::Embed),
            "completed" => Ok(IngestionStep::Completed),
            other => Err(ParseIngestionStepError(other.to_owned())),
        }
    }
}

/// The status of a single [`IngestionStepRun`], carrying the data that belongs
/// to each state so there are no conditional/optional fields: a run that has
/// started knows its `started_at`, a finished one its `completed_at`, and a
/// failed one its `error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepRunStatus {
    /// Created but not started yet.
    Pending,
    /// Currently executing, since `started_at`.
    Running { started_at: DateTime<Utc> },
    /// Finished successfully.
    Succeeded {
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    },
    /// Finished with an error.
    Failed {
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        error: String,
    },
    /// Stopped externally (via `stop`) before it finished.
    Aborted {
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    },
}

impl StepRunStatus {
    /// Whether the run is still in flight (pending or running). At most one
    /// active run may exist per job, which is how concurrent runs are prevented.
    pub fn is_active(&self) -> bool {
        matches!(self, StepRunStatus::Pending | StepRunStatus::Running { .. })
    }

    /// Whether the run finished successfully.
    pub fn is_succeeded(&self) -> bool {
        matches!(self, StepRunStatus::Succeeded { .. })
    }

    /// The storage/representation string of the status discriminant.
    pub fn as_str(&self) -> &'static str {
        match self {
            StepRunStatus::Pending => "pending",
            StepRunStatus::Running { .. } => "running",
            StepRunStatus::Succeeded { .. } => "succeeded",
            StepRunStatus::Failed { .. } => "failed",
            StepRunStatus::Aborted { .. } => "aborted",
        }
    }

    /// When the run started executing, if it has.
    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        match self {
            StepRunStatus::Pending => None,
            StepRunStatus::Running { started_at }
            | StepRunStatus::Succeeded { started_at, .. }
            | StepRunStatus::Failed { started_at, .. }
            | StepRunStatus::Aborted { started_at, .. } => Some(*started_at),
        }
    }

    /// When the run finished, if it has.
    pub fn completed_at(&self) -> Option<DateTime<Utc>> {
        match self {
            StepRunStatus::Pending | StepRunStatus::Running { .. } => None,
            StepRunStatus::Succeeded { completed_at, .. }
            | StepRunStatus::Failed { completed_at, .. }
            | StepRunStatus::Aborted { completed_at, .. } => Some(*completed_at),
        }
    }

    /// The failure message, if the run failed.
    pub fn error(&self) -> Option<&str> {
        match self {
            StepRunStatus::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// One execution of one [`IngestionStep`] for a job — the unit of the job's
/// run history. Each `restart` of a step records a fresh run with an incremented
/// [`attempt`](Self::attempt), so failures and retries are auditable.
///
/// A pure domain entity, deliberately not `Serialize`/`Deserialize`. The
/// lifecycle timestamps and failure message live in [`status`](Self::status),
/// not as optional fields here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestionStepRun {
    /// Stable identifier, assigned by the database on creation.
    pub id: Uuid,
    /// The job this run belongs to.
    pub job_id: Uuid,
    /// The step that was run.
    pub step: IngestionStep,
    /// 1-based attempt number for this `(job, step)`; incremented on each retry.
    pub attempt: i32,
    /// The run's state and the data that goes with it.
    pub status: StepRunStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [IngestionStep; 4] = [
        IngestionStep::Extract,
        IngestionStep::Chunk,
        IngestionStep::Embed,
        IngestionStep::Completed,
    ];

    #[test]
    fn as_str_round_trips_through_from_str() {
        for step in ALL {
            assert_eq!(IngestionStep::from_str(step.as_str()), Ok(step));
        }
    }

    #[test]
    fn from_str_rejects_unknown_steps() {
        assert_eq!(
            IngestionStep::from_str("review"),
            Err(ParseIngestionStepError("review".to_owned()))
        );
    }

    #[test]
    fn steps_advance_in_order_to_a_terminal_completed() {
        assert_eq!(IngestionStep::FIRST, IngestionStep::Extract);
        assert_eq!(IngestionStep::Extract.next(), IngestionStep::Chunk);
        assert_eq!(IngestionStep::Chunk.next(), IngestionStep::Embed);
        assert_eq!(IngestionStep::Embed.next(), IngestionStep::Completed);
        assert_eq!(IngestionStep::Completed.next(), IngestionStep::Completed);
    }

    #[test]
    fn only_completed_is_completed() {
        assert!(IngestionStep::Completed.is_completed());
        assert!(!IngestionStep::Extract.is_completed());
        assert!(!IngestionStep::Chunk.is_completed());
        assert!(!IngestionStep::Embed.is_completed());
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).expect("valid timestamp")
    }

    #[test]
    fn status_as_str_matches_the_variant() {
        assert_eq!(StepRunStatus::Pending.as_str(), "pending");
        assert_eq!(
            StepRunStatus::Running { started_at: at(1) }.as_str(),
            "running"
        );
        assert_eq!(
            StepRunStatus::Succeeded {
                started_at: at(1),
                completed_at: at(2)
            }
            .as_str(),
            "succeeded"
        );
        assert_eq!(
            StepRunStatus::Failed {
                started_at: at(1),
                completed_at: at(2),
                error: "boom".to_owned()
            }
            .as_str(),
            "failed"
        );
        assert_eq!(
            StepRunStatus::Aborted {
                started_at: at(1),
                completed_at: at(2)
            }
            .as_str(),
            "aborted"
        );
    }

    #[test]
    fn active_statuses_are_pending_and_running() {
        assert!(StepRunStatus::Pending.is_active());
        assert!(StepRunStatus::Running { started_at: at(1) }.is_active());
        assert!(
            !StepRunStatus::Succeeded {
                started_at: at(1),
                completed_at: at(2)
            }
            .is_active()
        );
        assert!(
            !StepRunStatus::Aborted {
                started_at: at(1),
                completed_at: at(2)
            }
            .is_active()
        );
    }

    #[test]
    fn only_succeeded_is_succeeded() {
        let succeeded = StepRunStatus::Succeeded {
            started_at: at(1),
            completed_at: at(2),
        };
        assert!(succeeded.is_succeeded());
        assert!(!StepRunStatus::Pending.is_succeeded());
        assert!(!StepRunStatus::Running { started_at: at(1) }.is_succeeded());
    }

    #[test]
    fn accessors_expose_the_variant_data() {
        let failed = StepRunStatus::Failed {
            started_at: at(1),
            completed_at: at(2),
            error: "boom".to_owned(),
        };
        assert_eq!(failed.started_at(), Some(at(1)));
        assert_eq!(failed.completed_at(), Some(at(2)));
        assert_eq!(failed.error(), Some("boom"));

        assert_eq!(StepRunStatus::Pending.started_at(), None);
        assert_eq!(StepRunStatus::Pending.completed_at(), None);
        assert_eq!(StepRunStatus::Pending.error(), None);
    }
}
