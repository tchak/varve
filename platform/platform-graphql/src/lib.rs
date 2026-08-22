//! The public GraphQL schema (`design/graphql.md`), executed in-process.
//!
//! **The schema never sees the transport** (P.7): every execution
//! receives an already-resolved [`Principal`] as request data — from
//! a browser session or an API token, the resolvers cannot tell —
//! and nothing here reads headers, cookies, or tokens. That is the
//! P.3 seam: `platform-app` owns transports and principal resolution;
//! this crate owns types and resolvers.
//!
//! **Current scope: the P0 slice** (G.6) — organizations, teams,
//! procedures, and their members, with the three `create*` mutations.
//! The type graph follows G.2: full objects only at root
//! ([`organization::Organization`], [`team::Team`],
//! [`procedure::Procedure`]), `*Ref` types everywhere a list or a
//! parent is named, `Member` as the account⟷container link. The
//! case-file types arrive with the kernel edge (P0's last step,
//! `varve-service`).
//!
//! Visibility is membership (G.6): every root lookup answers `null`
//! for an absent *or invisible* object, so an id never reveals
//! whether it exists; mutations fail with the structured errors in
//! [`error`].

#![forbid(unsafe_code)]

pub mod error;
pub mod member;
pub mod mutation;
pub mod organization;
pub mod procedure;
pub mod query;
pub mod team;

use async_graphql::{Context, EmptySubscription, Schema};
use platform_core::Principal;

pub use mutation::Mutation;
pub use query::Query;

/// The executable schema.
pub type PlatformSchema = Schema<Query, Mutation, EmptySubscription>;

/// Builds the schema. One per process, registered as app context by
/// `platform-app`.
pub fn schema() -> PlatformSchema {
    Schema::build(Query, Mutation, EmptySubscription).finish()
}

/// Executes `request` as `principal` over `db`. The one execution
/// entry point: the principal rides in as request data, so a request
/// can never reach a resolver unauthenticated — the type of this
/// function is the guard's last line.
pub async fn execute(
    schema: &PlatformSchema,
    request: async_graphql::Request,
    principal: Principal,
    db: toasty::Db,
) -> async_graphql::Response {
    schema.execute(request.data(principal).data(db)).await
}

/// The principal and a database handle, as every resolver reads them
/// from the request data. Both are present by construction —
/// [`execute`] is the only entry point and always attaches them; a
/// miss is a wiring bug, reported as [`error::Code::Internal`] rather
/// than a panic.
pub(crate) fn session<'a>(ctx: &Context<'a>) -> async_graphql::Result<(&'a Principal, toasty::Db)> {
    let principal = ctx
        .data::<Principal>()
        .map_err(|e| error::internal(e.message))?;
    let db = ctx
        .data::<toasty::Db>()
        .map_err(|e| error::internal(e.message))?
        .clone();
    Ok((principal, db))
}

/// Parses a GraphQL `ID` as a UUID; malformed ids are
/// [`error::Code::InvalidInput`].
pub(crate) fn parse_id(id: &async_graphql::ID) -> async_graphql::Result<uuid::Uuid> {
    uuid::Uuid::parse_str(id.as_str())
        .map_err(|_| error::invalid_input(format!("malformed id {:?}", id.as_str())))
}
