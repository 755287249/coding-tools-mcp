use axum::routing::{get, post};
use axum::Router;

use super::{
    mcp_delete, mcp_get, mcp_info, mcp_post, oauth_authorization_server_metadata,
    oauth_authorize_get, oauth_authorize_post, oauth_protected_resource_metadata,
    oauth_register_post, oauth_token_post, ListenerState,
};

pub(super) fn build_router(state: ListenerState) -> Router {
    let public_prefix = configured_route_prefix(&state.configured_public_url);
    let mut router = service_routes_for_prefix("")
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_authorization_server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth_protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth_protected_resource_metadata),
        );
    if !public_prefix.is_empty() {
        let authorization_metadata = authorization_metadata_path(&public_prefix);
        let protected_metadata = protected_resource_metadata_path(&public_prefix);
        router = router
            .merge(service_routes_for_prefix(&public_prefix))
            .route(
                &authorization_metadata,
                get(oauth_authorization_server_metadata),
            )
            .route(&protected_metadata, get(oauth_protected_resource_metadata));
    }
    let router = router.with_state(state);
    #[cfg(feature = "desktop")]
    let router = router.merge(crate::browser_share::router());
    router
}

fn service_routes_for_prefix(prefix: &str) -> Router<ListenerState> {
    let mcp = prefixed_route(prefix, "/mcp");
    let mcp_info_path = prefixed_route(prefix, "/mcp/info");
    let authorize = prefixed_route(prefix, "/oauth/authorize");
    let token = prefixed_route(prefix, "/oauth/token");
    let register = prefixed_route(prefix, "/oauth/register");

    Router::new()
        .route(&mcp, get(mcp_get).post(mcp_post).delete(mcp_delete))
        .route(&mcp_info_path, get(mcp_info))
        .route(
            &authorize,
            get(oauth_authorize_get).post(oauth_authorize_post),
        )
        .route(&token, post(oauth_token_post))
        .route(&register, post(oauth_register_post))
}

pub(super) fn authorization_metadata_path(prefix: &str) -> String {
    format!(
        "/.well-known/oauth-authorization-server{}",
        prefix.trim_end_matches('/')
    )
}

pub(super) fn protected_resource_metadata_path(prefix: &str) -> String {
    format!(
        "/.well-known/oauth-protected-resource{}/mcp",
        prefix.trim_end_matches('/')
    )
}

pub(super) fn configured_route_prefix(configured_public_url: &str) -> String {
    reqwest::Url::parse(configured_public_url.trim())
        .ok()
        .map(|url| url.path().trim_end_matches('/').to_string())
        .filter(|path| !path.is_empty() && path != "/")
        .unwrap_or_default()
}

pub(super) fn prefixed_route(prefix: &str, route: &str) -> String {
    if prefix.is_empty() {
        route.to_string()
    } else {
        format!("{}{}", prefix.trim_end_matches('/'), route)
    }
}
