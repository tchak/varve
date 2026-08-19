//! Tier 5 (§7, §13.6): the blob store — content-addressed byte storage
//! for attachments and resolver payload snapshots (§2.15: one machinery,
//! two clients). First crate in the workspace where IO and async are
//! legal.
//!
//! The trait is plaintext-streaming on both sides; encryption is an
//! implementation's concern (§13.2). Implementations that encrypt take
//! a [`Keyring`] — get-or-create one X25519 identity per blob, sole
//! recipient (PLATFORM.md P.10, P.9 Q7: shreddable) — the same
//! dependency inversion as `TableSink`: the platform implements it over
//! its database; this crate never sees one.
//!
//! [`FsBlobStore`] is the local-filesystem implementation, running the
//! same age pipeline as the future S3 impl (P.9 Q7: parity — keyring
//! and shred paths are exercised in dev and tests). v1 buffers blobs in
//! memory around the sync age API; the chunked streaming pipeline
//! (P.10 range serving) is the recorded follow-up and changes no
//! signatures.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use age::secrecy::ExposeSecret;
use age::x25519;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek};
use varve_core::canonical::{ContentHash, HashAlg};

/// Blob-level scan bookkeeping (§13.6): distinct from the kernel's
/// per-element scan status — this exists so one shared blob is scanned
/// once, not once per record; `varve-service` propagates verdicts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlobScan {
    pub engine: String,
    /// Signature database version the verdict was produced against.
    pub signatures: String,
    pub at_unix_secs: u64,
    pub verdict: ScanVerdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScanVerdict {
    Clean,
    Infected,
    Failed,
}

/// The manifest entry (§13.6, fields pinned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInfo {
    pub hash: ContentHash,
    pub byte_size: u64,
    /// Verified at ingest by the gateway (§2.15); recorded here.
    pub content_type: String,
    pub created_at: SystemTime,
    pub scan: Option<BlobScan>,
}

/// Inputs minted by the caller (§2.13: timestamps are inputs, at Tier 5
/// as everywhere).
#[derive(Debug, Clone)]
pub struct PutMeta {
    pub content_type: String,
    pub created_at: SystemTime,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub deleted: Vec<ContentHash>,
    pub kept_live: u64,
    /// Younger than the grace window (§13.6: the put-during-sweep race
    /// and the upload-slot orphan story are both this counter).
    pub kept_young: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("keyring backend: {0}")]
    Backend(String),
    #[error("no identity for {0}")]
    Missing(ContentHash),
}

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Keyring(#[from] KeyringError),
    #[error("blob not found: {0}")]
    NotFound(ContentHash),
    #[error("encryption: {0}")]
    Encrypt(String),
    #[error("decryption: {0}")]
    Decrypt(String),
    #[error("corrupt store: {0}")]
    Corrupt(String),
}

/// Per-blob key custody (P.10): one X25519 identity per blob, sole
/// recipient. `shred` deletes the key row — after it, the ciphertext is
/// unreadable everywhere, provider backups included.
pub trait Keyring: Send + Sync {
    fn identity_for(
        &self,
        hash: &ContentHash,
    ) -> impl Future<Output = Result<x25519::Identity, KeyringError>> + Send;

    /// Identity of an already-stored blob; `Missing` if shredded.
    fn existing_identity(
        &self,
        hash: &ContentHash,
    ) -> impl Future<Output = Result<x25519::Identity, KeyringError>> + Send;

    fn shred(&self, hash: &ContentHash) -> impl Future<Output = Result<(), KeyringError>> + Send;
}

/// The blob trait (§13.6). Plaintext on both sides; hashes are computed
/// by the store, never trusted from the caller — claim verification is
/// the contract (§2.15).
pub trait BlobStore: Send + Sync {
    /// Streams bytes in, returns the computed address. Idempotent by
    /// hash: re-putting existing content stores nothing — that is the
    /// §2.15 dedup path.
    fn put(
        &self,
        meta: PutMeta,
        reader: impl AsyncRead + Send + Unpin,
    ) -> impl Future<Output = Result<ContentHash, FilesError>> + Send;

    /// Seekable so ranges can be served (P.10).
    fn get(
        &self,
        hash: &ContentHash,
    ) -> impl Future<Output = Result<impl AsyncRead + AsyncSeek + Send + Unpin, FilesError>> + Send;

    fn has(&self, hash: &ContentHash) -> impl Future<Output = Result<bool, FilesError>> + Send;

    fn stat(&self, hash: &ContentHash)
    -> impl Future<Output = Result<BlobInfo, FilesError>> + Send;

    /// Blob-level bookkeeping (§13.6); the kernel's per-element scan
    /// status is separate and stays in `varve-record`.
    fn record_scan(
        &self,
        hash: &ContentHash,
        scan: BlobScan,
    ) -> impl Future<Output = Result<(), FilesError>> + Send;

    /// Key row first (shred), object second: the order that fails safe
    /// (§13.6).
    fn delete(&self, hash: &ContentHash) -> impl Future<Output = Result<(), FilesError>> + Send;

    /// §2.15 mark-and-sweep given roots: deletes blobs not in `live`
    /// and older than `grace`. `now` is an input, like every timestamp.
    fn sweep(
        &self,
        live: &BTreeSet<ContentHash>,
        grace: Duration,
        now: SystemTime,
    ) -> impl Future<Output = Result<SweepReport, FilesError>> + Send;
}

/// Reference keyring over an in-memory map: what the platform's
/// database-backed impl must behave like, and what tests use.
#[derive(Default)]
pub struct MemoryKeyring {
    keys: std::sync::Mutex<std::collections::BTreeMap<ContentHash, String>>,
}

impl MemoryKeyring {
    pub fn len(&self) -> usize {
        self.keys.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn parse(s: &str) -> Result<x25519::Identity, KeyringError> {
        s.parse::<x25519::Identity>().map_err(|e| KeyringError::Backend(e.to_string()))
    }
}

impl Keyring for MemoryKeyring {
    async fn identity_for(&self, hash: &ContentHash) -> Result<x25519::Identity, KeyringError> {
        let mut keys = self.keys.lock().expect("poisoned");
        if let Some(s) = keys.get(hash) {
            return Self::parse(s);
        }
        let identity = x25519::Identity::generate();
        keys.insert(*hash, identity.to_string().expose_secret().to_string());
        Ok(identity)
    }

    async fn existing_identity(&self, hash: &ContentHash) -> Result<x25519::Identity, KeyringError> {
        let keys = self.keys.lock().expect("poisoned");
        match keys.get(hash) {
            Some(s) => Self::parse(s),
            None => Err(KeyringError::Missing(*hash)),
        }
    }

    async fn shred(&self, hash: &ContentHash) -> Result<(), KeyringError> {
        self.keys.lock().expect("poisoned").remove(hash);
        Ok(())
    }
}

/// On-disk manifest record. Serde here is a storage format of a Tier 5
/// store, not a kernel internal (§9's guardrail concerns tiers 0–4).
#[derive(serde::Serialize, serde::Deserialize)]
struct ManifestRecord {
    hash: String,
    byte_size: u64,
    content_type: String,
    created_at_unix_secs: u64,
    scan: Option<BlobScan>,
}

/// Local-filesystem store: `<root>/blobs/<alg>-<hex>` (age ciphertext),
/// `<root>/manifest/<alg>-<hex>.json`.
pub struct FsBlobStore<K> {
    root: PathBuf,
    keyring: K,
}

fn stem(hash: &ContentHash) -> String {
    hash.to_string().replace(':', "-")
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

impl<K: Keyring> FsBlobStore<K> {
    pub async fn open(root: impl Into<PathBuf>, keyring: K) -> Result<Self, FilesError> {
        let root = root.into();
        tokio::fs::create_dir_all(root.join("blobs")).await?;
        tokio::fs::create_dir_all(root.join("manifest")).await?;
        Ok(Self { root, keyring })
    }

    fn blob_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join("blobs").join(stem(hash))
    }

    fn manifest_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join("manifest").join(format!("{}.json", stem(hash)))
    }

    async fn read_manifest(&self, hash: &ContentHash) -> Result<ManifestRecord, FilesError> {
        match tokio::fs::read(self.manifest_path(hash)).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| FilesError::Corrupt(format!("manifest for {hash}: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(FilesError::NotFound(*hash))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn write_manifest(
        &self,
        hash: &ContentHash,
        record: &ManifestRecord,
    ) -> Result<(), FilesError> {
        let bytes = serde_json::to_vec(record).expect("manifest serializes");
        tokio::fs::write(self.manifest_path(hash), bytes).await?;
        Ok(())
    }
}

fn encrypt(recipient: &x25519::Recipient, plain: &[u8]) -> Result<Vec<u8>, FilesError> {
    let encryptor = age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
        .map_err(|e| FilesError::Encrypt(e.to_string()))?;
    let mut out = Vec::new();
    let mut writer =
        encryptor.wrap_output(&mut out).map_err(|e| FilesError::Encrypt(e.to_string()))?;
    writer.write_all(plain).map_err(|e| FilesError::Encrypt(e.to_string()))?;
    writer.finish().map_err(|e| FilesError::Encrypt(e.to_string()))?;
    Ok(out)
}

fn decrypt(identity: &x25519::Identity, cipher: &[u8]) -> Result<Vec<u8>, FilesError> {
    let decryptor =
        age::Decryptor::new(cipher).map_err(|e| FilesError::Decrypt(e.to_string()))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| FilesError::Decrypt(e.to_string()))?;
    let mut plain = Vec::new();
    reader.read_to_end(&mut plain).map_err(|e| FilesError::Decrypt(e.to_string()))?;
    Ok(plain)
}

impl<K: Keyring> BlobStore for FsBlobStore<K> {
    async fn put(
        &self,
        meta: PutMeta,
        mut reader: impl AsyncRead + Send + Unpin,
    ) -> Result<ContentHash, FilesError> {
        let mut plain = Vec::new();
        reader.read_to_end(&mut plain).await?;

        // Blobs hash plain — the stated §2.15 dedup exception.
        let mut hasher = Sha256::new();
        hasher.update(&plain);
        let hash = ContentHash { alg: HashAlg::Sha256, digest: hasher.finalize().into() };

        if self.has(&hash).await? {
            return Ok(hash); // dedup: already stored, nothing to do
        }

        let identity = self.keyring.identity_for(&hash).await?;
        let cipher = encrypt(&identity.to_public(), &plain)?;
        tokio::fs::write(self.blob_path(&hash), cipher).await?;
        self.write_manifest(
            &hash,
            &ManifestRecord {
                hash: hash.to_string(),
                byte_size: plain.len() as u64,
                content_type: meta.content_type,
                created_at_unix_secs: unix(meta.created_at),
                scan: None,
            },
        )
        .await?;
        Ok(hash)
    }

    async fn get(
        &self,
        hash: &ContentHash,
    ) -> Result<impl AsyncRead + AsyncSeek + Send + Unpin, FilesError> {
        self.read_manifest(hash).await?;
        let cipher = tokio::fs::read(self.blob_path(hash)).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FilesError::NotFound(*hash)
            } else {
                FilesError::Io(e)
            }
        })?;
        let identity = self.keyring.existing_identity(hash).await?;
        let plain = decrypt(&identity, &cipher)?;
        Ok(std::io::Cursor::new(plain))
    }

    async fn has(&self, hash: &ContentHash) -> Result<bool, FilesError> {
        Ok(tokio::fs::try_exists(self.manifest_path(hash)).await?)
    }

    async fn stat(&self, hash: &ContentHash) -> Result<BlobInfo, FilesError> {
        let record = self.read_manifest(hash).await?;
        Ok(BlobInfo {
            hash: *hash,
            byte_size: record.byte_size,
            content_type: record.content_type,
            created_at: UNIX_EPOCH + Duration::from_secs(record.created_at_unix_secs),
            scan: record.scan,
        })
    }

    async fn record_scan(&self, hash: &ContentHash, scan: BlobScan) -> Result<(), FilesError> {
        let mut record = self.read_manifest(hash).await?;
        record.scan = Some(scan);
        self.write_manifest(hash, &record).await
    }

    async fn delete(&self, hash: &ContentHash) -> Result<(), FilesError> {
        // Key row first: even if object removal fails, the bytes are
        // already unreadable (§13.6).
        self.keyring.shred(hash).await?;
        for path in [self.manifest_path(hash), self.blob_path(hash)] {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    async fn sweep(
        &self,
        live: &BTreeSet<ContentHash>,
        grace: Duration,
        now: SystemTime,
    ) -> Result<SweepReport, FilesError> {
        let mut report = SweepReport::default();
        let mut entries = tokio::fs::read_dir(self.root.join("manifest")).await?;
        let mut candidates = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
                continue;
            };
            let Ok(hash) = stem.replacen('-', ":", 1).parse::<ContentHash>() else {
                return Err(FilesError::Corrupt(format!("unparseable manifest name {stem}")));
            };
            candidates.push(hash);
        }
        for hash in candidates {
            if live.contains(&hash) {
                report.kept_live += 1;
                continue;
            }
            let info = self.stat(&hash).await?;
            let age_of = now.duration_since(info.created_at).unwrap_or(Duration::ZERO);
            if age_of < grace {
                report.kept_young += 1; // §13.6: young blobs are protected
                continue;
            }
            self.delete(&hash).await?;
            report.deleted.push(hash);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_766_000_000)
    }

    fn meta() -> PutMeta {
        PutMeta { content_type: "application/pdf".into(), created_at: now() }
    }

    async fn store(dir: &tempfile::TempDir) -> FsBlobStore<MemoryKeyring> {
        FsBlobStore::open(dir.path(), MemoryKeyring::default()).await.unwrap()
    }

    async fn read_all(store: &FsBlobStore<MemoryKeyring>, hash: &ContentHash) -> Vec<u8> {
        let mut reader = store.get(hash).await.unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn round_trip_and_ciphertext_at_rest() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let plain = b"bonjour, piece jointe".to_vec();
        let hash = store.put(meta(), plain.as_slice()).await.unwrap();

        assert_eq!(read_all(&store, &hash).await, plain);

        // On disk: age ciphertext, never the plaintext.
        let on_disk = std::fs::read(store.blob_path(&hash)).unwrap();
        assert!(on_disk.starts_with(b"age-encryption.org/v1"));
        assert!(!on_disk.windows(plain.len()).any(|w| w == plain));

        let info = store.stat(&hash).await.unwrap();
        assert_eq!(info.byte_size, plain.len() as u64);
        assert_eq!(info.content_type, "application/pdf");
        assert_eq!(info.created_at, now());
    }

    #[tokio::test]
    async fn put_is_idempotent_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let a = store.put(meta(), &b"same bytes"[..]).await.unwrap();
        let b = store.put(meta(), &b"same bytes"[..]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(store.keyring.len(), 1);
        assert_eq!(std::fs::read_dir(dir.path().join("blobs")).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn delete_shreds_key_row_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let hash = store.put(meta(), &b"to erase"[..]).await.unwrap();
        assert_eq!(store.keyring.len(), 1);

        store.delete(&hash).await.unwrap();
        assert!(store.keyring.is_empty());
        assert!(!store.has(&hash).await.unwrap());
        assert!(matches!(store.get(&hash).await.err(), Some(FilesError::NotFound(_))));
    }

    #[tokio::test]
    async fn shredded_key_makes_ciphertext_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let hash = store.put(meta(), &b"backup scenario"[..]).await.unwrap();
        // Simulate: object survives (a provider backup), key row gone.
        store.keyring.shred(&hash).await.unwrap();
        assert!(matches!(
            store.get(&hash).await.err(),
            Some(FilesError::Keyring(KeyringError::Missing(_)))
        ));
    }

    #[tokio::test]
    async fn sweep_respects_roots_and_grace() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let old = PutMeta { content_type: "text/plain".into(), created_at: now() };
        let referenced = store.put(old.clone(), &b"referenced"[..]).await.unwrap();
        let orphaned = store.put(old, &b"orphaned"[..]).await.unwrap();
        let young_meta =
            PutMeta { content_type: "text/plain".into(), created_at: now() + Duration::from_secs(3000) };
        let young = store.put(young_meta, &b"just uploaded"[..]).await.unwrap();

        let live = BTreeSet::from([referenced]);
        let at = now() + Duration::from_secs(3600);
        let report = store.sweep(&live, Duration::from_secs(1800), at).await.unwrap();

        assert_eq!(report.deleted, vec![orphaned]);
        assert_eq!(report.kept_live, 1);
        assert_eq!(report.kept_young, 1);
        assert!(store.has(&referenced).await.unwrap());
        assert!(store.has(&young).await.unwrap());
        assert!(!store.has(&orphaned).await.unwrap());
        assert_eq!(store.keyring.len(), 2);
    }

    #[tokio::test]
    async fn scan_bookkeeping_persists() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let hash = store.put(meta(), &b"scan me"[..]).await.unwrap();
        assert_eq!(store.stat(&hash).await.unwrap().scan, None);

        let scan = BlobScan {
            engine: "clamav".into(),
            signatures: "27500".into(),
            at_unix_secs: unix(now()),
            verdict: ScanVerdict::Clean,
        };
        store.record_scan(&hash, scan.clone()).await.unwrap();
        assert_eq!(store.stat(&hash).await.unwrap().scan, Some(scan));
    }
}
