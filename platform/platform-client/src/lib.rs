//! The typed client for the public GraphQL schema (`design/graphql.md`).
//!
//! **Transport-agnostic by construction.** An operation is a
//! `{query, variables}` JSON document and a response a `{data,
//! errors}` document; [`Transport`] moves one to the other and
//! nothing else. Two transports exist: `http::Http` (bearer
//! token, `http` feature) for integrators, and `platform-graphql`'s
//! in-process one for the app's own components and the resolver
//! tests — both cross the same serialization boundary, so what the
//! app sees is byte-for-byte what an integrator sees (platform P.1
//! rule 4).
//!
//! **Fragments are structs.** Each `#[derive(cynic::QueryFragment)]`
//! is validated against `schema.graphql` at build time (P.9 Q2); the
//! app declares a component's fragment beside the component and a
//! page composes fragments by nesting them. This crate holds the
//! operations integrators and tests share.

#![forbid(unsafe_code)]

use std::future::Future;

use serde::de::DeserializeOwned;

pub mod organization;
pub mod procedure;
pub mod revision_draft;
pub mod team;
pub mod viewer;

#[cfg(feature = "http")]
pub mod http;

/// The schema markers cynic generates from `schema.graphql`.
#[cynic::schema("platform")]
pub mod schema {}

// `DateTime` (RFC 3339, UTC — G.4) is a `jiff::Timestamp` on both
// sides of the wire.
cynic::impl_scalar!(jiff::Timestamp, schema::DateTime);

/// `Slug`: an organization's URL/API handle. The server normalizes
/// and validates on input, so the client carries it as an opaque
/// string.
#[derive(cynic::Scalar, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Slug")]
pub struct Slug(pub String);

/// Moves a request document to a response document. Implementations
/// never inspect either: the transport is the bearer of bytes, not a
/// party to the operation.
pub trait Transport {
    /// Executes one `{query, variables, operationName}` document and
    /// returns the `{data, errors}` document.
    fn execute(
        &self,
        request: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, Error>> + Send;
}

/// Runs `operation` over `transport` and decodes the typed data.
/// Any GraphQL error is [`Error::GraphQl`]; partial data next to
/// errors is discarded — the schema's errors are structured failures
/// (G.2.7), never advisory.
pub async fn run<T, Q, V>(transport: &T, operation: cynic::Operation<Q, V>) -> Result<Q, Error>
where
    T: Transport + ?Sized,
    Q: DeserializeOwned,
    V: serde::Serialize,
{
    let request = serde_json::to_value(&operation)?;
    let response = transport.execute(request).await?;
    let response: cynic::GraphQlResponse<Q, Extensions> = serde_json::from_value(response)?;
    match response.errors {
        Some(errors) if !errors.is_empty() => Err(Error::GraphQl(errors)),
        _ => response.data.ok_or(Error::NoData),
    }
}

/// The `extensions` of a structured error (G.2.7).
#[derive(serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Extensions {
    /// The machine-readable code, as serialized; [`Code::parse`]
    /// types it.
    pub code: Option<String>,
}

/// A GraphQL error as the schema emits it.
pub type GraphQlError = cynic::GraphQlError<Extensions>;

/// The machine-readable codes a client may branch on — the client's
/// mirror of the server's `error::Code`; a server test keeps the two
/// sets equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// An argument failed validation before any use case ran.
    InvalidInput,
    /// The principal may not perform the mutation on this target —
    /// the same answer whether the target exists or not.
    Forbidden,
    /// `createOrganization`: the slug is already taken.
    SlugTaken,
    /// A revision-draft edit refused by the draft or the kernel; the
    /// message says why and the draft is unchanged.
    InvalidEdit,
    /// The target changed since it was read: re-read and retry.
    Conflict,
    /// A lifecycle transition refused from the procedure's current
    /// state; nothing changed.
    InvalidTransition,
    /// The platform failed, not the request.
    Internal,
}

impl Code {
    /// Every code, for the server-side set-equality test.
    pub const ALL: [Code; 7] = [
        Code::InvalidInput,
        Code::Forbidden,
        Code::SlugTaken,
        Code::InvalidEdit,
        Code::Conflict,
        Code::InvalidTransition,
        Code::Internal,
    ];

    /// The serialized form, `SCREAMING_SNAKE_CASE`.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::InvalidInput => "INVALID_INPUT",
            Code::Forbidden => "FORBIDDEN",
            Code::SlugTaken => "SLUG_TAKEN",
            Code::InvalidEdit => "INVALID_EDIT",
            Code::Conflict => "CONFLICT",
            Code::InvalidTransition => "INVALID_TRANSITION",
            Code::Internal => "INTERNAL",
        }
    }

    /// Types a serialized code; `None` for one this client predates.
    pub fn parse(code: &str) -> Option<Code> {
        Code::ALL.into_iter().find(|c| c.as_str() == code)
    }
}

/// What can go wrong running an operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The transport could not deliver the document.
    #[error("transport: {0}")]
    Transport(String),
    /// A document was not valid JSON of the expected shape.
    #[error("invalid document: {0}")]
    Decode(#[from] serde_json::Error),
    /// The server answered with errors.
    #[error("graphql: {}", messages(.0))]
    GraphQl(Vec<GraphQlError>),
    /// The server answered with neither data nor errors.
    #[error("response carried neither data nor errors")]
    NoData,
}

impl Error {
    /// The first error's typed code, when this is a [`Error::GraphQl`]
    /// with a code this client knows.
    pub fn code(&self) -> Option<Code> {
        let Error::GraphQl(errors) = self else {
            return None;
        };
        errors
            .first()?
            .extensions
            .as_ref()?
            .code
            .as_deref()
            .and_then(Code::parse)
    }
}

fn messages(errors: &[GraphQlError]) -> String {
    errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}
