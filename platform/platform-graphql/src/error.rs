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
    /// A revision-draft edit the kernel or the draft refuses: unknown
    /// element or parent, anchor outside its parent, or a schema the
    /// kernel's validation rejects (duplicate id, nesting beyond
    /// policy). The message carries the reason; the draft is unchanged.
    InvalidEdit,
    /// The target changed since the client last read it (two editors
    /// or administrators racing): re-read and retry.
    Conflict,
    /// A lifecycle transition the state machine refuses from the
    /// procedure's current state (closing a draft, reopening an open
    /// procedure). The message carries the refusal; nothing changed.
    InvalidTransition,
    /// The draft cannot publish as-is: nothing is in progress, or a
    /// choice has no options (G.7 — a legal draft state, refused at
    /// publication). Fix the draft and retry.
    InvalidDraft,
    /// The platform failed, not the request: a store error or a
    /// wiring bug. The cause is logged server-side and never
    /// serialized — a client learns nothing about the database.
    Internal,
}

impl Code {
    /// Every code; `platform-client` mirrors this set and a test
    /// keeps the two equal.
    pub const ALL: [Code; 8] = [
        Code::InvalidInput,
        Code::Forbidden,
        Code::SlugTaken,
        Code::InvalidEdit,
        Code::Conflict,
        Code::InvalidTransition,
        Code::InvalidDraft,
        Code::Internal,
    ];

    /// The serialized form.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::InvalidInput => "INVALID_INPUT",
            Code::Forbidden => "FORBIDDEN",
            Code::SlugTaken => "SLUG_TAKEN",
            Code::InvalidEdit => "INVALID_EDIT",
            Code::Conflict => "CONFLICT",
            Code::InvalidTransition => "INVALID_TRANSITION",
            Code::InvalidDraft => "INVALID_DRAFT",
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
