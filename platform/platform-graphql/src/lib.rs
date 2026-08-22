//! The public GraphQL schema (`design/graphql.md`), executed in-process.
//!
//! **The schema never sees the transport** (P.7): every execution
//! receives an already-resolved [`Principal`] as request data — from
//! a browser session or an API token, the resolvers cannot tell —
//! and nothing here reads headers, cookies, or tokens. That is the
//! P.3 seam: `platform-app` owns transports and principal resolution;
//! this crate owns types and resolvers.
//!
//! **Current scope: the walking skeleton's edge.** One query,
//! `viewer`, echoing the principal — enough to prove the transport
//! and the guard end to end through HTTP. The procedure / case-file
//! types arrive with the kernel edge (P0's last step, `varve-service`).

#![forbid(unsafe_code)]

use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema};
use platform_core::Principal;

/// The executable schema.
pub type PlatformSchema = Schema<Query, EmptyMutation, EmptySubscription>;

/// Builds the schema. One per process, registered as app context by
/// `platform-app`.
pub fn schema() -> PlatformSchema {
    Schema::build(Query, EmptyMutation, EmptySubscription).finish()
}

/// Executes `request` as `principal`. The one execution entry point:
/// the principal rides in as request data, so a request can never
/// reach a resolver unauthenticated — the type of this function is
/// the guard's last line.
pub async fn execute(
    schema: &PlatformSchema,
    request: async_graphql::Request,
    principal: Principal,
) -> async_graphql::Response {
    schema.execute(request.data(principal)).await
}

/// The query root.
pub struct Query;

#[Object]
impl Query {
    /// The authenticated account the request executes as.
    async fn viewer<'a>(&self, ctx: &Context<'a>) -> async_graphql::Result<Viewer<'a>> {
        // Present by construction — `execute` is the only entry point
        // and it always attaches a principal. A miss is a wiring bug.
        let principal = ctx.data::<Principal>()?;
        Ok(Viewer { principal })
    }
}

/// The principal as the schema exposes it (P0: the account-level
/// core — id, email, locale preference).
pub struct Viewer<'a> {
    principal: &'a Principal,
}

#[Object]
impl Viewer<'_> {
    /// The account id.
    async fn account_id(&self) -> uuid::Uuid {
        self.principal.account_id
    }

    /// The account's normalized email.
    async fn email(&self) -> &str {
        &self.principal.email
    }

    /// The account's locale preference, when one was chosen.
    async fn locale(&self) -> Option<&str> {
        self.principal.locale.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> Principal {
        Principal {
            account_id: uuid::Uuid::nil(),
            email: "viewer@example.test".to_owned(),
            locale: Some("fr".to_owned()),
        }
    }

    #[tokio::test]
    async fn viewer_echoes_the_principal() {
        let schema = schema();
        let request = async_graphql::Request::new("{ viewer { accountId email locale } }");
        let response = execute(&schema, request, principal()).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = serde_json::to_value(response.data).unwrap();
        assert_eq!(
            data,
            serde_json::json!({
                "viewer": {
                    "accountId": "00000000-0000-0000-0000-000000000000",
                    "email": "viewer@example.test",
                    "locale": "fr"
                }
            })
        );
    }

    #[test]
    fn sdl_names_the_viewer() {
        let sdl = schema().sdl();
        assert!(sdl.contains("type Viewer"), "{sdl}");
        assert!(sdl.contains("viewer: Viewer!"), "{sdl}");
    }
}
