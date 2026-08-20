//! The resolved identity every request-handling layer works with.
//!
//! P.7: browser sessions in the app and API tokens for integrators
//! both resolve to the same `Principal` before execution — the
//! GraphQL schema never sees the transport. The full shape carries
//! party ids, surface assignments, and platform roles; **P0 needs
//! only the account-level core**, because the kernel edge
//! (`varve-service`, surfaces) is wired in last under P.8's
//! outside-in ordering. The kernel-facing fields join this struct
//! with that integration.

use crate::account::Account;

/// The account-level identity a session or API token resolves to.
///
/// Party ids, surface assignments, and platform roles are absent by
/// design at P0 — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// The authenticated account's id.
    pub account_id: uuid::Uuid,
    /// The account's normalized (lowercased) email.
    pub email: String,
    /// The account's locale preference, if one was chosen. Locale
    /// resolution order (`Accept-Language` versus this preference)
    /// is `platform-app`'s business (P.3: no crate below it knows
    /// what a locale is — here it is an opaque stored string).
    pub locale: Option<String>,
}

impl Principal {
    /// Builds the P0 principal for an authenticated account.
    pub fn from_account(account: &Account) -> Self {
        Self {
            account_id: account.id,
            email: account.email.clone(),
            locale: account.locale.clone(),
        }
    }
}
