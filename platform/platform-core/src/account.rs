//! Accounts and credential verification (P.7's account-level core).
//!
//! Passwords are stored as Argon2id PHC strings (`argon2` with its
//! default parameters — the RustCrypto-recommended baseline); the
//! PHC format self-describes algorithm, version, parameters, and
//! salt, so parameter upgrades verify old hashes transparently.
//!
//! **Email normalization happens here, in the service functions, not
//! in the model or the database.** [`register`] and
//! [`verify_credentials`] both trim and Unicode-lowercase the email
//! before touching storage, so the column only ever holds normalized
//! values and the plain `#[unique]` index is effectively
//! case-insensitive — without a database-side expression index,
//! which toasty's derived schema cannot express. The invariant holds
//! only as long as every write path goes through this module; that
//! is exactly the P.3 use-case-service discipline.

use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{
        Error as PasswordHashError, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
        rand_core::OsRng,
    },
};

/// A platform account: the unit both browser sessions and API tokens
/// authenticate as (P.7).
#[derive(Debug, toasty::Model)]
pub struct Account {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// Normalized (trimmed, Unicode-lowercased) email — see the
    /// module docs for why normalization lives in the service
    /// functions. Unique across the platform.
    #[unique]
    pub email: String,

    /// Display name.
    pub name: String,

    /// Argon2id password hash in PHC string format
    /// (`$argon2id$v=19$...`). Never a raw password.
    pub password_hash: String,

    /// Locale preference carried into [`crate::Principal`]; `None`
    /// until the account picks one. Opaque here — resolution is
    /// `platform-app`'s job (P.3).
    pub locale: Option<String>,

    /// Set on insert.
    #[auto]
    pub created_at: jiff::Timestamp,

    /// Set on insert and on every update.
    #[auto]
    pub updated_at: jiff::Timestamp,
}

/// Infrastructure failure during an auth operation: the database, or
/// password-hash handling (a corrupt stored PHC string, an OS RNG
/// failure). Never signals "wrong credentials" — that is the `None`
/// arm of [`verify_credentials`].
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The underlying store failed.
    #[error("database error: {0}")]
    Db(#[from] toasty::Error),
    /// Hashing, salting, or parsing a stored hash failed.
    #[error("password hash error: {0}")]
    PasswordHash(#[from] PasswordHashError),
}

/// Failure modes of [`register`].
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    /// An account with this (normalized) email already exists. A
    /// typed outcome, not a panic or an opaque driver error: the
    /// insert races are settled by the database's unique index (see
    /// [`register`]).
    #[error("an account with this email already exists")]
    EmailTaken,
    /// Infrastructure failure.
    #[error(transparent)]
    Auth(#[from] AuthError),
}

/// Normalizes an email for storage and lookup: trim surrounding
/// whitespace, Unicode-lowercase. Both [`register`] and
/// [`verify_credentials`] apply this, which is what keeps the
/// `#[unique]` index case-insensitive in effect (module docs).
fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Hashes a password to a PHC string with Argon2id default
/// parameters and a fresh random salt.
fn hash_password(password: &str) -> Result<String, PasswordHashError> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

/// Verifies `password` against a stored PHC string. `Ok(false)` is
/// "wrong password"; `Err` is a malformed stored hash.
fn verify_password(password: &str, phc: &str) -> Result<bool, PasswordHashError> {
    let parsed = PasswordHash::new(phc)?;
    match Argon2::default().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(PasswordHashError::Password) => Ok(false),
        Err(err) => Err(err),
    }
}

/// A hash to verify against when the email is unknown, so the
/// unknown-email path performs the same Argon2 work as the
/// wrong-password path — see [`verify_credentials`]. Computed once
/// per process (same default parameters as real hashes, so the cost
/// matches).
fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password("varve-dummy-password").expect("hashing a constant password cannot fail")
    })
}

/// Registers a new account: normalizes the email, hashes the
/// password, inserts.
///
/// Duplicate detection is race-free: the insert is an
/// insert-or-ignore against the `email` unique index
/// (`upsert_by_email(..).or_ignore()`, `ON CONFLICT DO NOTHING` on
/// PostgreSQL), so two concurrent registrations of the same email
/// cannot both succeed and the loser gets
/// [`RegisterError::EmailTaken`] rather than an opaque driver error.
pub async fn register(
    db: &mut toasty::Db,
    email: &str,
    password: &str,
    name: &str,
) -> Result<Account, RegisterError> {
    let email = normalize_email(email);
    let password_hash = hash_password(password).map_err(AuthError::from)?;
    let created = Account::upsert_by_email(&email)
        .name(name)
        .password_hash(&password_hash)
        .or_ignore()
        .exec(db)
        .await
        .map_err(AuthError::from)?;
    created.ok_or(RegisterError::EmailTaken)
}

/// Checks an email/password pair. `Ok(Some(account))` on success;
/// `Ok(None)` for **both** unknown email and wrong password, so the
/// caller cannot leak which one failed.
///
/// The two failure paths are also kept close in timing: on unknown
/// email this still runs one Argon2 verification against a dummy
/// hash (with the same parameters as real ones), instead of
/// returning early after a cheap index miss — otherwise response
/// time would betray email existence. (Database lookup time still
/// differs slightly; the dominant cost, the memory-hard hash, is
/// what's equalized.)
pub async fn verify_credentials(
    db: &mut toasty::Db,
    email: &str,
    password: &str,
) -> Result<Option<Account>, AuthError> {
    let email = normalize_email(email);
    let account = Account::filter_by_email(&email).first().exec(db).await?;
    match account {
        Some(account) => {
            if verify_password(password, &account.password_hash)? {
                Ok(Some(account))
            } else {
                Ok(None)
            }
        }
        None => {
            // Burn the same hash-verification cost as the
            // wrong-password path; the result is irrelevant.
            let _ = verify_password(password, dummy_hash());
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trip() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &phc).unwrap());
    }

    #[test]
    fn wrong_password_rejected() {
        let phc = hash_password("right").unwrap();
        assert!(!verify_password("wrong", &phc).unwrap());
    }

    #[test]
    fn phc_format_is_argon2id_v19() {
        // The stored format is load-bearing: verification parses the
        // PHC string, and platform-app may one day inspect the
        // algorithm tag for rehash-on-login. Pin it.
        let phc = hash_password("x").unwrap();
        assert!(
            phc.starts_with("$argon2id$v=19$"),
            "unexpected PHC prefix: {phc}"
        );
    }

    #[test]
    fn hashes_are_salted() {
        assert_ne!(
            hash_password("same").unwrap(),
            hash_password("same").unwrap()
        );
    }

    #[test]
    fn email_normalization() {
        assert_eq!(
            normalize_email("  Alice@Example.COM \n"),
            "alice@example.com"
        );
        // Unicode lowercase, not ASCII-only.
        assert_eq!(normalize_email("ÉLODIE@exemple.fr"), "élodie@exemple.fr");
        assert_eq!(normalize_email("already@lower.case"), "already@lower.case");
    }

    #[test]
    fn malformed_stored_hash_is_an_error_not_a_mismatch() {
        assert!(verify_password("x", "not-a-phc-string").is_err());
    }

    #[test]
    fn dummy_hash_verifies_like_a_real_one() {
        // The timing-equalization path must exercise a real Argon2id
        // verification, not fail fast on a parse error.
        assert!(!verify_password("some other password", dummy_hash()).unwrap());
    }
}
