//! `/graphql`, derived from this module's name: the public API's
//! transport (PLATFORM.md P.3 — "`/graphql` is an ordinary topcoat
//! `#[route]`; execution is `schema.execute` in-process").
//!
//! **Bearer only.** This route authenticates with `Authorization:
//! Bearer <api token>` and nothing else: the browser session cookie
//! is ignored here even when present. Integrators hold tokens, the
//! app's own components execute documents in-process (not over
//! HTTP), and a route no cookie can authenticate has no CSRF surface
//! at all — which is why the guard is the *whole* of the route's
//! security, and why it is a `#[route]` (no HTML layout, no shell).
//!
//! A request without a live token answers `401` with
//! `WWW-Authenticate: Bearer realm="varve"` and a GraphQL-shaped
//! error body, before any parsing of the query — an unauthenticated
//! client learns nothing about the schema. The same answer covers
//! missing, malformed, unknown, revoked, and expired tokens.

use topcoat::{
    context::{Cx, app_context},
    router::{
        HeaderValue, StatusCode,
        content::Json,
        header,
        response::{IntoResponse, Response},
        route,
    },
};

use crate::auth::bearer_principal;

/// The `WWW-Authenticate` challenge every 401 carries (RFC 6750 §3).
const CHALLENGE: HeaderValue = HeaderValue::from_static("Bearer realm=\"varve\"");

/// Executes one GraphQL request as the bearer token's principal.
#[route(POST)]
pub async fn submit(
    cx: &Cx,
    Json(request): Json<async_graphql::Request>,
) -> topcoat::Result<Response> {
    let Some(principal) = bearer_principal(cx).await? else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, CHALLENGE)],
            Json(async_graphql::Response::from_errors(vec![
                async_graphql::ServerError::new("Unauthorized", None),
            ])),
        )
            .into_response(cx);
    };
    let schema = app_context::<platform_graphql::PlatformSchema>(cx);
    let response = platform_graphql::execute(schema, request, principal.clone()).await;
    Json(response).into_response(cx)
}
