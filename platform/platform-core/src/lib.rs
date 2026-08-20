//! Toasty models for platform-owned data (accounts, procedure
//! catalog, team membership, messages, API tokens, webhook subscriptions,
//! notification outbox) and the use-case services: each use case composes
//! one `varve-service` operation with its platform side effects in exactly
//! one place (PLATFORM.md P.3).

#![forbid(unsafe_code)]
