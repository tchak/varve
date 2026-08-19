//! §2.15: the scan lifecycle beside attachment cells — the scanner is
//! Tier 5; the kernel provides state and pure enumeration so surfaces
//! can gate on it.
//!
//! Scans are not lifecycle ops in the log (unlike resolutions, §2.8):
//! a verdict describes a blob, not the record, and is re-derivable by
//! rescanning. See PLATFORM.md P.11.

use varve_core::canonical::ContentHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// The attachment element id (§2.4 value-internal identity).
    pub element: String,
    pub hash: ContentHash,
    pub status: ScanStatus,
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Pending,
    Clean,
    Infected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal scan transition {from:?} → {to:?}")]
pub struct ScanTransitionError {
    pub from: ScanStatus,
    pub to: ScanStatus,
}

impl Scan {
    /// `pending → clean | infected | failed`; `failed → pending`
    /// (retry, counted). Clean and infected are terminal.
    pub fn transition(&mut self, to: ScanStatus) -> Result<(), ScanTransitionError> {
        use ScanStatus::*;
        let legal = matches!(
            (self.status, to),
            (Pending, Clean | Infected | Failed) | (Failed, Pending)
        );
        if !legal {
            return Err(ScanTransitionError {
                from: self.status,
                to,
            });
        }
        if (self.status, to) == (Failed, Pending) {
            self.attempts += 1;
        }
        self.status = to;
        Ok(())
    }
}

pub fn pending_scans(scans: &[Scan]) -> Vec<&Scan> {
    scans
        .iter()
        .filter(|s| s.status == ScanStatus::Pending)
        .collect()
}
