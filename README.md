# Zyndeck

Zyndeck is an application for managing deckbuilding games. It lets you build
decks, validate them against each game's rules, and ask a built-in LLM
questions about how a game's rules work.

It is implemented as a Rust
[Cargo workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html);
each component lives in its own crate directory at the repository root.

## Crates

| Crate | Description |
| --- | --- |
| [`zyndeck-ingester`](zyndeck-ingester) | Service that ingests game rules so they can be validated against and queried by the LLM. |

## Requirements

- A Rust toolchain (edition 2024 — Rust 1.85 or newer; tested with 1.94).
  Install via [rustup](https://rustup.rs/).
- [`pre-commit`](https://pre-commit.com/) for the git hooks.

## Install

```bash
git clone https://github.com/leroyguillaume/zyndeck.git
cd zyndeck
cargo build
pre-commit install
```

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck_ingester=debug`). |

## Run

Run a specific crate from the workspace root with `-p`:

```bash
cargo run -p zyndeck-ingester
```

Or with a more verbose log filter:

```bash
RUST_LOG=zyndeck_ingester=debug cargo run -p zyndeck-ingester
```

The service runs until it receives `SIGINT` (Ctrl-C) or `SIGTERM`, then shuts
down gracefully.

## Test

Run the whole workspace test suite:

```bash
cargo test --workspace
```

Lint and format checks (also run as pre-commit hooks):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
