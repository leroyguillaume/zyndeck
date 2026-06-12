# zyndeck-core

The domain model of [Zyndeck](../README.md): the entities and value types the
rest of the workspace is built on.

It is **pure** — no I/O, no persistence, no HTTP. Other crates depend on these
types and map them to and from their own representations: [`zyndeck-db`](../zyndeck-db)
to database rows, [`zyndeck-api`](../zyndeck-api) to request/response DTOs. Domain
entities are deliberately **not** `Serialize`/`Deserialize`; serialization is the
boundary layers' concern.

## What's in it

| Type | Kind | Description |
| --- | --- | --- |
| `Game` | entity | A catalogued game, with a localised name. |
| `User` | entity | A user account with a `Role`. |
| `Role` | value type | `SuperAdmin` / `Admin` / `User`, with `FromStr`/`Display`. |
| `LocalizedString` | value type | A map of ISO 639-1 language code → text (e.g. a game name in several languages). |
| `LanguageCode` | value type | A validated ISO 639-1 language code. |
| `IngestionJob` | entity | One run of the rule-ingestion pipeline for a document. |
| `IngestionStep` | value type | A pipeline stage: `Extract` → `Chunk` → `Embed` → `Completed`. |
| `IngestionStepRun` | entity | One attempt of a step, with its `StepRunStatus`. |
| `StepRunStatus` | value type | A run's state (`Pending` / `Running` / `Succeeded` / `Failed` / `Aborted`), carrying the data each state owns. |

See the [`zyndeck-cli`](../zyndeck-cli#the-ingestion-job-model) and
[`zyndeck-ingester`](../zyndeck-ingester) READMEs for how the ingestion types
drive the pipeline.

## Test

```bash
cargo test -p zyndeck-core
```
