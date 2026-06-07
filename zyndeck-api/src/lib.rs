//! Zyndeck HTTP API: CRUD for games and users with role-based access control.

mod auth;
mod error;
mod game;
mod login;
mod openapi;
mod pagination;
mod password;
mod state;
mod user;

pub use error::ApiError;
pub use password::hash_password;
pub use state::AppState;

use std::sync::Arc;

use aide::axum::ApiRouter;
use aide::openapi::OpenApi;
use axum::response::Html;
use axum::routing::get;
use axum::{Extension, Json, Router};
use zyndeck_db::{GameRepository, UserRepository};

/// Builds the application router: the documented API, plus the OpenAPI document
/// at `/openapi.json` and a Scalar reference UI at `/docs`.
pub fn build_router<G, U>(state: AppState<G, U>) -> Router
where
    G: GameRepository + Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    aide::generate::extract_schemas(true);

    let mut api = openapi::base();
    ApiRouter::new()
        .merge(login::router::<G, U>())
        .merge(game::router::<G, U>())
        .merge(user::router::<G, U>())
        // Infrastructure endpoints stay out of the documented API surface.
        .route("/openapi.json", get(serve_openapi))
        .route("/docs", get(serve_docs))
        .finish_api_with(&mut api, openapi::transform)
        .layer(Extension(Arc::new(api)))
        .with_state(state)
}

async fn serve_openapi(Extension(api): Extension<Arc<OpenApi>>) -> Json<Arc<OpenApi>> {
    Json(api)
}

async fn serve_docs() -> Html<&'static str> {
    Html(SCALAR_HTML)
}

// Loads the current Scalar build from its CDN, pointed at our spec. (aide's
// bundled Scalar renders with broken CSS, so we host our own minimal page.)
const SCALAR_HTML: &str = r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Zyndeck API</title>
  </head>
  <body>
    <script id="api-reference" data-url="/openapi.json"></script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
  </body>
</html>
"#;
