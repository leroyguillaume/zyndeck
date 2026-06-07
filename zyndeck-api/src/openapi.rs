use aide::openapi::{Info, OpenApi, SecurityScheme, Tag};
use aide::transform::TransformOpenApi;

/// The base OpenAPI document (title/version), filled in further by [`transform`].
pub fn base() -> OpenApi {
    OpenApi {
        info: Info {
            title: "Zyndeck API".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            ..Info::default()
        },
        ..OpenApi::default()
    }
}

/// Declares tags (with order) and the bearer security scheme.
pub fn transform(api: TransformOpenApi) -> TransformOpenApi {
    api.title("Zyndeck API")
        .tag(Tag {
            name: "Authentication".to_owned(),
            description: Some("Obtain a bearer token.".to_owned()),
            ..Tag::default()
        })
        .tag(Tag {
            name: "Games".to_owned(),
            description: Some("Game catalogue — public reads, admin writes.".to_owned()),
            ..Tag::default()
        })
        .tag(Tag {
            name: "Users".to_owned(),
            description: Some("User accounts — admin-managed.".to_owned()),
            ..Tag::default()
        })
        .security_scheme(
            "BearerAuth",
            SecurityScheme::Http {
                scheme: "bearer".to_owned(),
                bearer_format: Some("JWT".to_owned()),
                description: Some("HS256-signed JWT whose `sub` is the user id.".to_owned()),
                extensions: Default::default(),
            },
        )
}
