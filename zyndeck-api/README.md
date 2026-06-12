# zyndeck-api

The HTTP API of [Zyndeck](../README.md): CRUD for games and users, with JWT
authentication and role-based access control.

The binary is a server. It connects to the database via
[`zyndeck-db`](../zyndeck-db), **applies any outstanding migrations at startup**,
ensures a bootstrap super-admin exists, then serves the API until it receives
`SIGINT`/`SIGTERM` and shuts down gracefully.

The API is built with `axum` + `aide`: every endpoint is documented and the
generated OpenAPI document is served at **`/openapi.json`**, with an interactive
[Scalar](https://scalar.com/) reference at **`/docs`**.

## Endpoints

| Method & path | Access | Description |
| --- | --- | --- |
| `POST /auth/login` | public | Exchange credentials for a JWT. |
| `GET /games` | public | List games (paginated). |
| `GET /games/{id}` | public | Get one game. |
| `POST /games` | admin | Create a game. |
| `PUT /games/{id}` | admin | Update a game. |
| `DELETE /games/{id}` | admin | Delete a game. |
| `GET /users` | authenticated | List users (paginated). |
| `GET /users/{id}` | authenticated | Get one user. |
| `POST /users` | admin | Create a user. |
| `PUT /users/{id}` | admin | Update a user. |
| `DELETE /users/{id}` | admin | Delete a user. |

List endpoints are paginated. Infrastructure routes (`/openapi.json`, `/docs`)
are intentionally kept out of the documented API surface.

## Authentication

`POST /auth/login` issues an HS256 JWT. Pass it as `Authorization: Bearer <token>`
on protected endpoints; Scalar shows a lock icon and a token input for them.
Mutations on games and users require an **admin** (or super-admin) role; reading
games is public, reading users requires any authenticated user.

A bootstrap **super-admin** is created (or its password reset) on every startup
from `ADMIN_USERNAME` / `ADMIN_PASSWORD`, so a fresh database is always
reachable.

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck_api=debug`). |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |
| `--bind-addr` | `BIND_ADDR` | `0.0.0.0:8080` | Address the HTTP server binds to. |
| `--jwt-secret` | `JWT_SECRET` | _(required)_ | Secret used to sign and verify HS256 JWTs. |
| `--jwt-ttl` | `JWT_TTL_SECONDS` | `86400` | Lifetime, in seconds, of issued tokens. |
| `--admin-username` | `ADMIN_USERNAME` | `admin` | Username of the bootstrap super-admin. |
| `--admin-password` | `ADMIN_PASSWORD` | _(required)_ | Password of the bootstrap super-admin. |

## Run

From the workspace root, with `-p`. The server needs a database (start the
compose Postgres first: `docker compose up -d postgres`):

```bash
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck

cargo run -p zyndeck-api -- \
  --jwt-secret change-me \
  --admin-password change-me
```

Then browse the API reference at <http://localhost:8080/docs>.

## Test

```bash
cargo test -p zyndeck-api
```
