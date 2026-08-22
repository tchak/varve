//! Structured errors (G.2.7): a GraphQL error whose `extensions.code`
//! names the failure; no `payload { errors }` result types anywhere.

use async_graphql::{Error, ErrorExtensions};

/// The machine-readable codes a client may branch on. Serialized as
/// `extensions.code`, `SCREAMING_SNAKE_CASE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// An argument failed validation before any use case ran
    /// (malformed id, empty name, …).
    InvalidInput,
    /// The principal may not perform the mutation on this target —
    /// the same answer whether the target exists or not (G.6).
    Forbidden,
    /// `createOrganization`: the slug is already taken.
    SlugTaken,
    /// The platform failed, not the request: a store error or a
    /// wiring bug. The cause is logged server-side and never
    /// serialized — a client learns nothing about the database.
    Internal,
}

impl Code {
    fn as_str(self) -> &'static str {
        match self {
            Code::InvalidInput => "INVALID_INPUT",
            Code::Forbidden => "FORBIDDEN",
            Code::SlugTaken => "SLUG_TAKEN",
            Code::Internal => "INTERNAL",
        }
    }
}

/// An error with `extensions.code = code`.
pub fn coded(code: Code, message: impl Into<String>) -> Error {
    Error::new(message.into()).extend_with(|_, e| e.set("code", code.as_str()))
}

/// An [`Code::InvalidInput`] error.
pub fn invalid_input(message: impl Into<String>) -> Error {
    coded(Code::InvalidInput, message)
}

/// The one [`Code::Forbidden`] error: deliberately uninformative so a
/// missing target and a foreign one read the same.
pub fn forbidden() -> Error {
    coded(Code::Forbidden, "forbidden")
}

/// The one [`Code::Internal`] error. `cause` is logged at `error`
/// level with its full source chain and replaced by a fixed message:
/// `toasty::Error`'s `Display` walks the driver chain (constraint and
/// table names, connection details), which is for operators only.
pub fn internal(cause: impl std::fmt::Display) -> Error {
    tracing::error!(error = %cause, "graphql resolver failed");
    coded(Code::Internal, "internal error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_masks_the_cause() {
        let error = internal("connection to server at \"db.internal\" failed");
        assert_eq!(error.message, "internal error");
        let extensions = error.extensions.expect("extensions");
        assert_eq!(
            extensions.get("code"),
            Some(&async_graphql::Value::from("INTERNAL"))
        );
    }
}
