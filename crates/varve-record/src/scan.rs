//! §2.15: the scan lifecycle beside attachment cells — mirroring
//! resolutions (§2.8), and aligned with them (2026-08-19): a **fold of
//! lifecycle ops carried by ordinary chained entries**, per attachment
//! element. The scanner is Tier 5 (PLATFORM.md P.11); the kernel
//! records *that* an element's bytes were submitted for scanning and
//! *how it ended*, and provides the pure pending-enumeration so surfaces
//! can gate on it. Attempts, transient scanner errors, backoff and
//! deadlines are scheduler state and never reach the record.

use std::collections::BTreeMap;

use varve_core::canonical::ContentHash;

use crate::resolution::{AbandonReason, Outcome};

/// A scan lifecycle transition, as carried by `EntryOp::Scan`:
///
/// ```text
/// pending → clean | infected | failed | abandoned
///           (any terminal state) → pending        (rescan: deliberate, recorded)
/// ```
///
/// `failed` is a **definitive** verdict — the bytes cannot be scanned
/// (an encrypted archive, an oversize file) — never a scanner outage;
/// those are scheduler state. A rescan against new signatures (P.11)
/// is a fresh `request`, bulk-able.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTransition {
    /// Submit the element's bytes: which bytes is bound here, as a
    /// resolution binds its versions (§2.8 rule 1).
    Request {
        hash: ContentHash,
    },
    Clean {
        outcome: Outcome,
    },
    Infected {
        /// The detection name, when the engine gives one.
        threat: Option<String>,
        outcome: Outcome,
    },
    Failed {
        outcome: Outcome,
    },
    Abandon {
        reason: AbandonReason,
        outcome: Outcome,
    },
}

impl ScanTransition {
    pub fn status(&self) -> ScanStatus {
        match self {
            ScanTransition::Request { .. } => ScanStatus::Pending,
            ScanTransition::Clean { .. } => ScanStatus::Clean,
            ScanTransition::Infected { .. } => ScanStatus::Infected,
            ScanTransition::Failed { .. } => ScanStatus::Failed,
            ScanTransition::Abandon { reason, .. } => ScanStatus::Abandoned(*reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Pending,
    Clean,
    Infected,
    Failed,
    Abandoned(AbandonReason),
}

impl ScanStatus {
    pub fn is_pending(self) -> bool {
        self == ScanStatus::Pending
    }
}

/// One scan instance — the fold of an element's scan ops. Keyed by the
/// attachment **element id** (§2.4 value-internal identity), which
/// outlives the element in the cell: a scan of a since-replaced file
/// stays in the fold as history. The platform ends a pending scan whose
/// element was removed (`abandon`, reason `superseded`), as it ends a
/// pending lookup whose input changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub element: String,
    /// The bytes submitted by the (latest) request.
    pub hash: ContentHash,
    pub status: ScanStatus,
    /// Seq of the entry carrying the (latest) `request`.
    pub requested_at: u64,
    /// Seq of the entry carrying the terminal transition, if any.
    pub closed_at: Option<u64>,
    /// The detection name of an `infected` verdict, when given.
    pub threat: Option<String>,
    /// The terminal transition's summary, if the scan is closed.
    pub outcome: Option<Outcome>,
}

/// A scan op the fold refuses (the table above); `append` rejects it
/// like an op that does not apply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanLifecycleError {
    /// `request` while the element's scan is pending: end it first.
    #[error("scan of element {element} is already pending; end it before rescanning")]
    AlreadyPending { element: String },
    /// A verdict on an element whose scan is not pending — never
    /// requested, or already closed.
    #[error("scan of element {element} is not pending (status {status:?})")]
    NotPending {
        element: String,
        status: Option<ScanStatus>,
    },
}

/// Fold one scan transition into `scans`.
pub(crate) fn fold_scan_transition(
    scans: &mut BTreeMap<String, Scan>,
    seq: u64,
    element: &str,
    transition: &ScanTransition,
) -> Result<(), ScanLifecycleError> {
    let current = scans.get(element).map(|s| s.status);
    match transition {
        ScanTransition::Request { hash } => {
            if current.is_some_and(|s| s.is_pending()) {
                return Err(ScanLifecycleError::AlreadyPending {
                    element: element.to_string(),
                });
            }
            scans.insert(
                element.to_string(),
                Scan {
                    element: element.to_string(),
                    hash: *hash,
                    status: ScanStatus::Pending,
                    requested_at: seq,
                    closed_at: None,
                    threat: None,
                    outcome: None,
                },
            );
            Ok(())
        }
        terminal => {
            let Some(s) = scans.get_mut(element).filter(|s| s.status.is_pending()) else {
                return Err(ScanLifecycleError::NotPending {
                    element: element.to_string(),
                    status: current,
                });
            };
            s.status = terminal.status();
            s.closed_at = Some(seq);
            s.outcome = Some(match terminal {
                ScanTransition::Clean { outcome }
                | ScanTransition::Infected { outcome, .. }
                | ScanTransition::Failed { outcome }
                | ScanTransition::Abandon { outcome, .. } => outcome.clone(),
                ScanTransition::Request { .. } => unreachable!("handled above"),
            });
            if let ScanTransition::Infected { threat, .. } = terminal {
                s.threat = threat.clone();
            }
            Ok(())
        }
    }
}
