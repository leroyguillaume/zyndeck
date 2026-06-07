//! Integration tests for the HTTP API, focused on the authorization matrix.
//!
//! The database is mocked (`zyndeck-db`'s `mock` feature), so these tests need
//! no Postgres: each test wires the exact repository expectations for the one
//! request it makes, builds the real router, and drives it with `tower`'s
//! `oneshot`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use futures::stream;
use http_body_util::BodyExt;
use serde::Serialize;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use zyndeck_api::{AppState, build_router, hash_password};
use zyndeck_core::{Game, LocalizedString, Role, User};
use zyndeck_db::{Credentials, Error, MockGameRepository, MockUserRepository};

// HS256 requires a key of at least 32 bytes.
const SECRET: &str = "0123456789abcdef0123456789abcdef";
const TOKEN_TTL: u64 = 3600;

#[derive(Serialize)]
struct Claims {
    sub: String,
    exp: usize,
}

/// Mints a valid HS256 token whose subject is `id`.
fn token_for(id: Uuid) -> String {
    let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &Claims {
            sub: id.to_string(),
            exp,
        },
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("mint token")
}

fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(0, 0).unwrap()
}

fn make_user(username: &str, role: Role) -> User {
    User {
        id: Uuid::new_v4(),
        username: username.to_owned(),
        role,
        created_at: epoch(),
    }
}

fn make_game(creator: Uuid) -> Game {
    Game {
        id: Uuid::new_v4(),
        name: LocalizedString::from_pairs([("en", "Marvel Champions")]).unwrap(),
        created_at: epoch(),
        created_by: creator,
    }
}

/// Makes the users mock answer `find_by_id(user.id)` with `user` (covers the
/// auth lookup and, where applicable, a self-fetch — hence `times(1..)`).
fn expect_find_user(users: &mut MockUserRepository, user: &User) {
    let id = user.id;
    let user = user.clone();
    users
        .expect_find_by_id()
        .withf(move |queried| *queried == id)
        .times(1..)
        .returning(move |_| {
            let user = user.clone();
            Box::pin(async move { Ok(Some(user)) })
        });
}

/// Builds the app from the given mocks and performs a single request.
async fn call(
    games: MockGameRepository,
    users: MockUserRepository,
    method: &str,
    uri: &str,
    caller: Option<Uuid>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let app: Router = build_router(AppState::with_repositories(games, users, SECRET, TOKEN_TTL));

    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(id) = caller {
        builder = builder.header("authorization", format!("Bearer {}", token_for(id)));
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())),
        None => builder.body(Body::empty()),
    }
    .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

// ---- Login ------------------------------------------------------------------

#[tokio::test]
async fn login_succeeds_with_correct_password() {
    let id = Uuid::new_v4();
    let hash = hash_password("password123").unwrap();
    let mut users = MockUserRepository::new();
    users
        .expect_find_credentials_by_username()
        .returning(move |_| {
            let credentials = Credentials {
                id,
                password_hash: hash.clone(),
            };
            Box::pin(async move { Ok(Some(credentials)) })
        });

    let (status, body) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/auth/login",
        None,
        Some(json!({ "username": "admin", "password": "password123" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["accessToken"].as_str().is_some_and(|t| !t.is_empty()));
}

#[tokio::test]
async fn login_rejects_a_wrong_password() {
    let hash = hash_password("password123").unwrap();
    let mut users = MockUserRepository::new();
    users
        .expect_find_credentials_by_username()
        .returning(move |_| {
            let credentials = Credentials {
                id: Uuid::new_v4(),
                password_hash: hash.clone(),
            };
            Box::pin(async move { Ok(Some(credentials)) })
        });

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/auth/login",
        None,
        Some(json!({ "username": "admin", "password": "wrong" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rejects_an_unknown_user() {
    let mut users = MockUserRepository::new();
    users
        .expect_find_credentials_by_username()
        .returning(|_| Box::pin(async { Ok(None) }));

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/auth/login",
        None,
        Some(json!({ "username": "ghost", "password": "password123" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---- Games: reads public, writes admin-only ---------------------------------

#[tokio::test]
async fn list_games_is_public() {
    let mut games = MockGameRepository::new();
    games.expect_count().returning(|| Box::pin(async { Ok(0) }));
    games
        .expect_list()
        .returning(|| Box::pin(stream::iter(Vec::<zyndeck_db::Result<Game>>::new())));

    let (status, body) = call(
        games,
        MockUserRepository::new(),
        "GET",
        "/games",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn get_game_is_public() {
    let creator = Uuid::new_v4();
    let game = make_game(creator);
    let returned = game.clone();
    let mut games = MockGameRepository::new();
    games.expect_find_by_id().returning(move |_| {
        let game = returned.clone();
        Box::pin(async move { Ok(Some(game)) })
    });

    let (status, body) = call(
        games,
        MockUserRepository::new(),
        "GET",
        &format!("/games/{}", game.id),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["creatorId"], creator.to_string());
}

#[tokio::test]
async fn create_game_requires_a_token() {
    let (status, _) = call(
        MockGameRepository::new(),
        MockUserRepository::new(),
        "POST",
        "/games",
        None,
        Some(json!({ "name": { "en": "Arkham Horror" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_game_is_forbidden_for_plain_users() {
    let user = make_user("joe", Role::User);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &user);

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/games",
        Some(user.id),
        Some(json!({ "name": { "en": "Arkham Horror" } })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_game_is_allowed_for_admins() {
    let admin = make_user("admin", Role::Admin);
    let admin_id = admin.id;
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);

    let mut games = MockGameRepository::new();
    games.expect_create().returning(|new_game| {
        let game = Game {
            id: Uuid::new_v4(),
            name: new_game.name,
            created_at: epoch(),
            created_by: new_game.created_by,
        };
        Box::pin(async move { Ok(game) })
    });

    let (status, body) = call(
        games,
        users,
        "POST",
        "/games",
        Some(admin_id),
        Some(json!({ "name": { "en": "Arkham Horror" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["creatorId"], admin_id.to_string());
}

#[tokio::test]
async fn delete_game_is_allowed_for_admins() {
    let admin = make_user("admin", Role::Admin);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);

    let mut games = MockGameRepository::new();
    games
        .expect_delete()
        .returning(|_| Box::pin(async { Ok(true) }));

    let (status, _) = call(
        games,
        users,
        "DELETE",
        &format!("/games/{}", Uuid::new_v4()),
        Some(admin.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ---- Users: protected reads, self-scope, admin writes -----------------------

#[tokio::test]
async fn list_users_requires_a_token() {
    let (status, _) = call(
        MockGameRepository::new(),
        MockUserRepository::new(),
        "GET",
        "/users",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_users_is_forbidden_for_plain_users() {
    let user = make_user("joe", Role::User);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &user);

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "GET",
        "/users",
        Some(user.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_users_is_allowed_for_admins() {
    let admin = make_user("admin", Role::Admin);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);
    users.expect_count().returning(|| Box::pin(async { Ok(0) }));
    users
        .expect_list()
        .returning(|| Box::pin(stream::iter(Vec::<zyndeck_db::Result<User>>::new())));

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "GET",
        "/users",
        Some(admin.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn plain_user_can_read_themselves() {
    let user = make_user("alice", Role::User);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &user);

    let (status, body) = call(
        MockGameRepository::new(),
        users,
        "GET",
        &format!("/users/{}", user.id),
        Some(user.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "alice");
}

#[tokio::test]
async fn plain_user_cannot_read_others() {
    let user = make_user("alice", Role::User);
    let other = Uuid::new_v4();
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &user);

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "GET",
        &format!("/users/{other}"),
        Some(user.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_read_anyone() {
    let admin = make_user("admin", Role::Admin);
    let target = make_user("bob", Role::User);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);
    expect_find_user(&mut users, &target);

    let (status, body) = call(
        MockGameRepository::new(),
        users,
        "GET",
        &format!("/users/{}", target.id),
        Some(admin.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "bob");
}

#[tokio::test]
async fn create_user_is_forbidden_for_plain_users() {
    let user = make_user("joe", Role::User);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &user);

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/users",
        Some(user.id),
        Some(json!({ "username": "newbie", "password": "password123", "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_user_is_allowed_for_admins() {
    let admin = make_user("admin", Role::Admin);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);
    users.expect_create().returning(|new_user| {
        let user = User {
            id: Uuid::new_v4(),
            username: new_user.username,
            role: new_user.role,
            created_at: epoch(),
        };
        Box::pin(async move { Ok(user) })
    });

    let (status, body) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/users",
        Some(admin.id),
        Some(json!({ "username": "newbie", "password": "password123", "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["username"], "newbie");
    assert_eq!(body["role"], "user");
}

#[tokio::test]
async fn duplicate_username_is_a_conflict() {
    let admin = make_user("admin", Role::Admin);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);
    users
        .expect_create()
        .returning(|_| Box::pin(async { Err(Error::UsernameTaken("newbie".to_owned())) }));

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "POST",
        "/users",
        Some(admin.id),
        Some(json!({ "username": "newbie", "password": "password123", "role": "user" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

// ---- Delete authorization ---------------------------------------------------

#[tokio::test]
async fn admin_deletes_a_plain_user() {
    let admin = make_user("admin", Role::Admin);
    let target = make_user("joe", Role::User);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);
    expect_find_user(&mut users, &target);
    users
        .expect_delete()
        .returning(|_| Box::pin(async { Ok(true) }));

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "DELETE",
        &format!("/users/{}", target.id),
        Some(admin.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn admin_cannot_delete_another_admin() {
    let admin = make_user("admin", Role::Admin);
    let target = make_user("boss", Role::Admin);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &admin);
    expect_find_user(&mut users, &target);

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "DELETE",
        &format!("/users/{}", target.id),
        Some(admin.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn super_admin_deletes_an_admin() {
    let root = make_user("root", Role::SuperAdmin);
    let target = make_user("admin", Role::Admin);
    let mut users = MockUserRepository::new();
    expect_find_user(&mut users, &root);
    expect_find_user(&mut users, &target);
    users
        .expect_delete()
        .returning(|_| Box::pin(async { Ok(true) }));

    let (status, _) = call(
        MockGameRepository::new(),
        users,
        "DELETE",
        &format!("/users/{}", target.id),
        Some(root.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
