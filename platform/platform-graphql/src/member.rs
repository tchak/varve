//! `Member`: one account's membership in a container (organization or
//! team). The container is the parent object — a member carries no
//! back-pointer (G.2 rule 1) — so the same type serves both.

use async_graphql::{ID, Object};

/// A membership link.
pub struct Member(pub platform_core::Member);

#[Object]
impl Member {
    /// The member account.
    async fn account(&self) -> AccountRef<'_> {
        AccountRef(&self.0.account)
    }

    /// When the membership was created.
    async fn joined_at(&self) -> jiff::Timestamp {
        self.0.joined_at
    }
}

/// An account as lists name it: id, display name, email. The email
/// is visible to co-members (G.6 — DN shows reviewers each other's
/// addresses).
pub struct AccountRef<'a>(pub &'a platform_core::Account);

#[Object]
impl AccountRef<'_> {
    async fn id(&self) -> ID {
        ID::from(self.0.id)
    }

    async fn name(&self) -> &str {
        &self.0.name
    }

    async fn email(&self) -> &str {
        &self.0.email
    }
}
