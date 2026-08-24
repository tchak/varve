//! The procedure lifecycle (P.4 *Procedure lifecycle*): a hand-rolled
//! three-state machine — `Draft` (never published) → `Published` ⇄
//! `Closed` — as a plain enum with fallible transition functions.
//! Three states and four transitions earn no state-machine crate,
//! and the case-file checkpoint machine will be the same shape.
//!
//! [`ProcedureState`] carries each state's one fact (`since`);
//! [`ProcedureStateValue`] is the bare discriminant stored on the
//! catalog row and mirrored by the API's filtering enum (G.2 rule 5).
//! Only the current state's fact lives on the row (`state_since`);
//! history is the event log's ([`crate::procedure_event`]). The
//! transitions are pure — `now` is an input — so the whole machine
//! is provable without a database; the services that persist a
//! transition live in [`crate::procedure`].

use jiff::Timestamp;

/// A procedure's lifecycle state with its fact. `since` means *since
/// when the procedure is open* (or closed) — deliberately not
/// `published_at`: [`ProcedureState::reopen`] resets it with no
/// publication happening, and revision publication timestamps are
/// kernel facts (`RevisionStore` publication events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcedureState {
    /// Never published. No fact of its own — `created_at` already
    /// lives on the row.
    Draft,
    /// Open for submissions since `since`.
    Published { since: Timestamp },
    /// Closed to new submissions since `since`.
    Closed { since: Timestamp },
}

/// The bare discriminant, as stored on the catalog row (a native
/// database enum) and filtered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, toasty::Embed)]
pub enum ProcedureStateValue {
    Draft,
    Published,
    Closed,
}

impl ProcedureState {
    /// Rebuilds the state from its row columns. The pairing (`since`
    /// is `None` iff `Draft`) is a storage invariant every write in
    /// this crate keeps; a mismatch is [`CorruptState`] — corruption
    /// to surface, never to repair silently.
    pub fn from_columns(
        value: ProcedureStateValue,
        since: Option<Timestamp>,
    ) -> Result<Self, CorruptState> {
        match (value, since) {
            (ProcedureStateValue::Draft, None) => Ok(Self::Draft),
            (ProcedureStateValue::Published, Some(since)) => Ok(Self::Published { since }),
            (ProcedureStateValue::Closed, Some(since)) => Ok(Self::Closed { since }),
            (value, since) => Err(CorruptState { value, since }),
        }
    }

    /// The row columns: the discriminant plus `state_since` (`None`
    /// iff `Draft`).
    pub fn columns(&self) -> (ProcedureStateValue, Option<Timestamp>) {
        match *self {
            Self::Draft => (ProcedureStateValue::Draft, None),
            Self::Published { since } => (ProcedureStateValue::Published, Some(since)),
            Self::Closed { since } => (ProcedureStateValue::Closed, Some(since)),
        }
    }

    /// Publication lands in `Published` from any state — from
    /// `Closed` it *is* the reopen (P.4). Publishing into an
    /// already-open procedure leaves `since` alone: the procedure
    /// never stopped being open.
    #[must_use]
    pub fn publish(self, now: Timestamp) -> Self {
        match self {
            Self::Published { since } => Self::Published { since },
            Self::Draft | Self::Closed { .. } => Self::Published { since: now },
        }
    }

    /// Closes to new submissions. Only from `Published`: a
    /// never-published procedure is deleted, not closed (G.2 rule 8).
    pub fn close(self, now: Timestamp) -> Result<Self, TransitionError> {
        match self {
            Self::Published { .. } => Ok(Self::Closed { since: now }),
            other => Err(TransitionError::NotPublished(other.columns().0)),
        }
    }

    /// Reopens on the last published revision — no kernel event, no
    /// publication; the transition that makes `since` not a
    /// publication date.
    pub fn reopen(self, now: Timestamp) -> Result<Self, TransitionError> {
        match self {
            Self::Closed { .. } => Ok(Self::Published { since: now }),
            other => Err(TransitionError::NotClosed(other.columns().0)),
        }
    }
}

/// A transition the machine refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("only a published procedure can close (state: {0:?})")]
    NotPublished(ProcedureStateValue),
    #[error("only a closed procedure can reopen (state: {0:?})")]
    NotClosed(ProcedureStateValue),
}

/// The stored `(state, state_since)` pair broke the `None` iff
/// `Draft` invariant — database-level corruption to surface, not
/// silently replace (the `RevisionDraftError::Corrupt` precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("stored lifecycle state is unreadable: {value:?} with state_since {since:?}")]
pub struct CorruptState {
    pub value: ProcedureStateValue,
    pub since: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn publish_opens_from_every_state() {
        let now = ts("2026-08-24T10:00:00Z");
        assert_eq!(
            ProcedureState::Draft.publish(now),
            ProcedureState::Published { since: now }
        );
        assert_eq!(
            ProcedureState::Closed {
                since: ts("2026-01-01T00:00:00Z")
            }
            .publish(now),
            ProcedureState::Published { since: now }
        );
    }

    #[test]
    fn publish_into_an_open_procedure_keeps_since() {
        let since = ts("2026-01-01T00:00:00Z");
        let now = ts("2026-08-24T10:00:00Z");
        assert_eq!(
            ProcedureState::Published { since }.publish(now),
            ProcedureState::Published { since }
        );
    }

    #[test]
    fn close_only_from_published() {
        let since = ts("2026-01-01T00:00:00Z");
        let now = ts("2026-08-24T10:00:00Z");
        assert_eq!(
            ProcedureState::Published { since }.close(now),
            Ok(ProcedureState::Closed { since: now })
        );
        assert_eq!(
            ProcedureState::Draft.close(now),
            Err(TransitionError::NotPublished(ProcedureStateValue::Draft))
        );
        assert_eq!(
            ProcedureState::Closed { since }.close(now),
            Err(TransitionError::NotPublished(ProcedureStateValue::Closed))
        );
    }

    #[test]
    fn reopen_only_from_closed_and_resets_since() {
        let since = ts("2026-01-01T00:00:00Z");
        let now = ts("2026-08-24T10:00:00Z");
        assert_eq!(
            ProcedureState::Closed { since }.reopen(now),
            Ok(ProcedureState::Published { since: now })
        );
        assert_eq!(
            ProcedureState::Draft.reopen(now),
            Err(TransitionError::NotClosed(ProcedureStateValue::Draft))
        );
        assert_eq!(
            ProcedureState::Published { since }.reopen(now),
            Err(TransitionError::NotClosed(ProcedureStateValue::Published))
        );
    }

    #[test]
    fn columns_round_trip_and_refuse_mismatches() {
        let since = ts("2026-01-01T00:00:00Z");
        for state in [
            ProcedureState::Draft,
            ProcedureState::Published { since },
            ProcedureState::Closed { since },
        ] {
            let (value, since) = state.columns();
            assert_eq!(ProcedureState::from_columns(value, since), Ok(state));
        }
        assert!(ProcedureState::from_columns(ProcedureStateValue::Draft, Some(since)).is_err());
        assert!(ProcedureState::from_columns(ProcedureStateValue::Published, None).is_err());
        assert!(ProcedureState::from_columns(ProcedureStateValue::Closed, None).is_err());
    }
}
