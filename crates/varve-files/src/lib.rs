//! Tier 5 (§7, §13.6): the blob store — content-addressed byte storage
//! for attachments and resolver payload snapshots (§2.15: one machinery,
//! two clients). First crate in the workspace where IO and async are
//! legal.
//!
//! The trait is plaintext-streaming on both sides; encryption is an
//! implementation's concern (§13.2). Implementations that encrypt take
//! a [`Keyring`] — one X25519 identity per blob, sole recipient
//! (PLATFORM.md P.10, P.9 Q7: shreddable) — the same dependency
//! inversion as `TableSink`: the platform implements it over its
//! database; this crate never sees one.
//!
//! [`ObjectBlobStore`] is the implementation — one impl, generic over
//! any [`object_store`] backend: local filesystem and in-memory for
//! dev and tests, S3-compatible providers (AWS, OVH, Scaleway, MinIO…)
//! in production. Swapping providers is a constructor, not code — and
//! dev runs the exact pipeline production runs (P.9 Q7: parity).
//!
//! `put` is a single streaming pass — hash ⊕ age-encrypt, 64 KiB
//! chunks, constant memory (the P.10 gateway tee, minus the scan leg
//! which lives at the gateway) — multipart-uploaded to a staging key,
//! then server-side copied to its content address. Because the
//! per-blob key is addressed by a hash only known once the bytes have
//! passed, the identity is generated ephemerally at put-start and
//! **registered** afterward: hence [`Keyring::register`] is
//! first-write-wins, not get-or-create. The registration is the commit
//! point of a `put`: a failure after it rolls the key row back, a
//! concurrent loser only claims success once the winner's blob is
//! readable (`PutInFlight` otherwise), and `sweep` shreds rows a crash
//! orphaned — a key row without a manifest must never outlive the
//! grace window, or the address is poisoned. Range access is a first-class
//! operation (`get_range`), not client-side seek: age implements seek
//! only on its sync reader, and remote backends serve ranges as ranged
//! GETs anyway (P.10) — decryption runs on a blocking thread over a
//! lazy ranged reader, streamed out through a duplex pipe.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::io::Read;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use age::secrecy::ExposeSecret;
use age::x25519;
use bytes::Bytes;
use futures::StreamExt;
use futures::io::AsyncWriteExt as FuturesWriteExt;
use object_store::buffered::BufWriter;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::compat::TokioAsyncWriteCompatExt;
use tokio_util::io::SyncIoBridge;
use varve_core::canonical::{ContentHash, HashAlg};

pub use object_store;

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
    /// Abandoned staging objects removed (a `put` that never completed).
    pub tmp_removed: u64,
    /// Crash-orphaned key rows shredded: registrations older than the
    /// grace window with no manifest behind them — a `put` that died
    /// between register and commit. Shredding heals the address.
    pub keys_removed: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("keyring backend: {0}")]
    Backend(String),
    #[error("no identity for {0}")]
    Missing(ContentHash),
    /// A concurrent `put` of the same content won the registration.
    #[error("identity already registered for {0}")]
    AlreadyRegistered(ContentHash),
}

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Storage(#[from] object_store::Error),
    #[error(transparent)]
    Keyring(#[from] KeyringError),
    #[error("blob not found: {0}")]
    NotFound(ContentHash),
    /// The address has a registered key but no stored blob behind it:
    /// a concurrent `put` of the same content is mid-commit, or a
    /// crashed one left an orphaned row (`sweep` reclaims those after
    /// the grace window). Never claimed as success — a `put` that
    /// returns `Ok` guarantees the blob is readable. Retryable.
    #[error("blob {0}: a concurrent put owns the address but has not committed")]
    PutInFlight(ContentHash),
    #[error("encryption: {0}")]
    Encrypt(String),
    #[error("decryption: {0}")]
    Decrypt(String),
    #[error("corrupt store: {0}")]
    Corrupt(String),
}

fn map_storage(e: object_store::Error, hash: &ContentHash) -> FilesError {
    match e {
        object_store::Error::NotFound { .. } => FilesError::NotFound(*hash),
        e => FilesError::Storage(e),
    }
}

/// Per-blob key custody (P.10): one X25519 identity per blob, sole
/// recipient. `shred` deletes the key row — after it, the ciphertext is
/// unreadable everywhere, provider backups included.
pub trait Keyring: Send + Sync {
    /// First-write-wins: the streaming pipeline generates the identity
    /// before the hash is known, and registers it after. On
    /// `AlreadyRegistered`, the caller's ciphertext is discarded — the
    /// concurrent writer's blob is the one the address names. `at`
    /// stamps the row (timestamps are inputs, §2.13) so `sweep` can
    /// tell a crashed put's orphan from a registration whose put is
    /// still committing.
    fn register(
        &self,
        hash: &ContentHash,
        identity: &x25519::Identity,
        at: SystemTime,
    ) -> impl Future<Output = Result<(), KeyringError>> + Send;

    /// Identity of an already-stored blob; `Missing` if shredded.
    fn existing_identity(
        &self,
        hash: &ContentHash,
    ) -> impl Future<Output = Result<x25519::Identity, KeyringError>> + Send;

    fn shred(&self, hash: &ContentHash) -> impl Future<Output = Result<(), KeyringError>> + Send;

    /// Hashes registered at or before `before` — `sweep`'s
    /// reconciliation view: a row that old with no manifest behind it
    /// is a crashed put's orphan, and shredding it heals the address.
    fn registered_before(
        &self,
        before: SystemTime,
    ) -> impl Future<Output = Result<Vec<ContentHash>, KeyringError>> + Send;
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

    fn get(
        &self,
        hash: &ContentHash,
    ) -> impl Future<Output = Result<impl AsyncRead + Send + Unpin, FilesError>> + Send;

    /// Range access as a first-class operation (P.10 — HTTP Range).
    /// A range reaching past the end is truncated, HTTP-style.
    fn get_range(
        &self,
        hash: &ContentHash,
        range: Range<u64>,
    ) -> impl Future<Output = Result<impl AsyncRead + Send + Unpin, FilesError>> + Send;

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
    /// and older than `grace`, plus abandoned staging objects. `now` is
    /// an input, like every timestamp.
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
    keys: std::sync::Mutex<std::collections::BTreeMap<ContentHash, (String, SystemTime)>>,
}

impl MemoryKeyring {
    pub fn len(&self) -> usize {
        self.keys.lock().expect("poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Keyring for MemoryKeyring {
    async fn register(
        &self,
        hash: &ContentHash,
        identity: &x25519::Identity,
        at: SystemTime,
    ) -> Result<(), KeyringError> {
        let mut keys = self.keys.lock().expect("poisoned");
        if keys.contains_key(hash) {
            return Err(KeyringError::AlreadyRegistered(*hash));
        }
        keys.insert(
            *hash,
            (identity.to_string().expose_secret().to_string(), at),
        );
        Ok(())
    }

    async fn existing_identity(
        &self,
        hash: &ContentHash,
    ) -> Result<x25519::Identity, KeyringError> {
        let keys = self.keys.lock().expect("poisoned");
        match keys.get(hash) {
            Some((s, _)) => s
                .parse::<x25519::Identity>()
                .map_err(|e| KeyringError::Backend(e.to_string())),
            None => Err(KeyringError::Missing(*hash)),
        }
    }

    async fn shred(&self, hash: &ContentHash) -> Result<(), KeyringError> {
        self.keys.lock().expect("poisoned").remove(hash);
        Ok(())
    }

    async fn registered_before(
        &self,
        before: SystemTime,
    ) -> Result<Vec<ContentHash>, KeyringError> {
        Ok(self
            .keys
            .lock()
            .expect("poisoned")
            .iter()
            .filter(|(_, (_, at))| *at <= before)
            .map(|(hash, _)| *hash)
            .collect())
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

fn stem(hash: &ContentHash) -> String {
    hash.to_string().replace(':', "-")
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// Sync `Read + Seek` over an object via ranged GETs, for age's
/// seekable decryption. Runs on a blocking thread; fetches lazily in
/// spans, so the access pattern age produces (header, one seek, then
/// sequential) costs a handful of requests, not one per chunk.
struct RangedObjectReader {
    store: Arc<dyn ObjectStore>,
    path: ObjPath,
    len: u64,
    pos: u64,
    handle: tokio::runtime::Handle,
    buf: Bytes,
    buf_start: u64,
}

const FETCH_SPAN: u64 = 4 * 1024 * 1024;

impl Read for RangedObjectReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len || out.is_empty() {
            return Ok(0);
        }
        let buf_end = self.buf_start + self.buf.len() as u64;
        if self.pos < self.buf_start || self.pos >= buf_end {
            let end = (self.pos + FETCH_SPAN).min(self.len);
            let range = self.pos..end;
            let fetched = self
                .handle
                .block_on(self.store.get_range(&self.path, range))
                .map_err(std::io::Error::other)?;
            self.buf_start = self.pos;
            self.buf = fetched;
        }
        let offset = (self.pos - self.buf_start) as usize;
        let n = out.len().min(self.buf.len() - offset);
        out[..n].copy_from_slice(&self.buf[offset..offset + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for RangedObjectReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        use std::io::SeekFrom::*;
        let target = match pos {
            Start(n) => n as i128,
            End(n) => self.len as i128 + n as i128,
            Current(n) => self.pos as i128 + n as i128,
        };
        if target < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = target as u64;
        Ok(self.pos)
    }
}

/// The blob store, generic over any `object_store` backend. Layout:
/// `blobs/<alg>-<hex>` (age ciphertext), `manifest/<alg>-<hex>.json`,
/// `tmp/` for in-flight staging.
pub struct ObjectBlobStore<K> {
    store: Arc<dyn ObjectStore>,
    keyring: K,
    tmp_counter: AtomicU64,
}

impl<K: Keyring> ObjectBlobStore<K> {
    pub fn new(store: Arc<dyn ObjectStore>, keyring: K) -> Self {
        Self {
            store,
            keyring,
            tmp_counter: AtomicU64::new(0),
        }
    }

    /// Local-filesystem backend (dev, tests): same pipeline, same
    /// layout, different constructor — that is the provider swap.
    pub fn local(root: impl Into<PathBuf>, keyring: K) -> Result<Self, FilesError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(root)?;
        Ok(Self::new(Arc::new(store), keyring))
    }

    /// In-memory backend (tests).
    pub fn memory(keyring: K) -> Self {
        Self::new(Arc::new(object_store::memory::InMemory::new()), keyring)
    }

    fn blob_path(hash: &ContentHash) -> ObjPath {
        ObjPath::from(format!("blobs/{}", stem(hash)))
    }

    fn manifest_path(hash: &ContentHash) -> ObjPath {
        ObjPath::from(format!("manifest/{}.json", stem(hash)))
    }

    async fn read_manifest(&self, hash: &ContentHash) -> Result<ManifestRecord, FilesError> {
        let result = self
            .store
            .get(&Self::manifest_path(hash))
            .await
            .map_err(|e| map_storage(e, hash))?;
        let bytes = result.bytes().await.map_err(|e| map_storage(e, hash))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| FilesError::Corrupt(format!("manifest for {hash}: {e}")))
    }

    async fn write_manifest(
        &self,
        hash: &ContentHash,
        record: &ManifestRecord,
    ) -> Result<(), FilesError> {
        let bytes = serde_json::to_vec(record).expect("manifest serializes");
        self.store
            .put(&Self::manifest_path(hash), Bytes::from(bytes).into())
            .await?;
        Ok(())
    }

    /// Sync age decryption (the only seekable reader age offers) on a
    /// blocking thread over ranged GETs, streamed out through a duplex
    /// pipe: constant memory, setup errors surfaced before the stream
    /// is returned; mid-stream corruption surfaces as truncation.
    async fn decrypt_stream(
        &self,
        hash: &ContentHash,
        range: Option<Range<u64>>,
    ) -> Result<tokio::io::DuplexStream, FilesError> {
        self.read_manifest(hash).await?;
        let identity = self.keyring.existing_identity(hash).await?;
        let path = Self::blob_path(hash);
        let meta = self
            .store
            .head(&path)
            .await
            .map_err(|e| map_storage(e, hash))?;
        let reader = RangedObjectReader {
            store: self.store.clone(),
            path,
            len: meta.size,
            pos: 0,
            handle: tokio::runtime::Handle::current(),
            buf: Bytes::new(),
            buf_start: 0,
        };
        let (read_half, write_half) = tokio::io::duplex(64 * 1024);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let setup = (|| {
                let decryptor =
                    age::Decryptor::new(reader).map_err(|e| FilesError::Decrypt(e.to_string()))?;
                let mut reader = decryptor
                    .decrypt(std::iter::once(&identity as &dyn age::Identity))
                    .map_err(|e| FilesError::Decrypt(e.to_string()))?;
                if let Some(r) = &range {
                    use std::io::Seek;
                    reader.seek(std::io::SeekFrom::Start(r.start))?;
                }
                Ok::<_, FilesError>(reader)
            })();
            match setup {
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
                Ok(reader) => {
                    let _ = ready_tx.send(Ok(()));
                    let mut bridge = SyncIoBridge::new(write_half);
                    let _ = match range {
                        Some(r) => {
                            let mut limited = reader.take(r.end.saturating_sub(r.start));
                            std::io::copy(&mut limited, &mut bridge)
                        }
                        None => {
                            let mut reader = reader;
                            std::io::copy(&mut reader, &mut bridge)
                        }
                    };
                }
            }
        });
        ready_rx
            .await
            .map_err(|_| FilesError::Corrupt("decrypt task vanished".into()))??;
        Ok(read_half)
    }
}

impl<K: Keyring> BlobStore for ObjectBlobStore<K> {
    async fn put(
        &self,
        meta: PutMeta,
        mut reader: impl AsyncRead + Send + Unpin,
    ) -> Result<ContentHash, FilesError> {
        // The identity must exist before the first encrypted byte, but
        // the hash it will be filed under is only known at the end —
        // ephemeral now, registered after (module docs).
        let identity = x25519::Identity::generate();
        let recipient = identity.to_public();

        let staging = ObjPath::from(format!(
            "tmp/put-{}-{}",
            std::process::id(),
            self.tmp_counter.fetch_add(1, Ordering::Relaxed)
        ));

        // One pass: hash ⊕ encrypt, 64 KiB chunks, constant memory,
        // multipart-uploaded to the staging key as it streams.
        let mut hasher = Sha256::new();
        let mut byte_size: u64 = 0;
        let upload = BufWriter::new(self.store.clone(), staging.clone());
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .map_err(|e| FilesError::Encrypt(e.to_string()))?;
        let mut writer = encryptor
            .wrap_async_output(upload.compat_write())
            .await
            .map_err(|e| FilesError::Encrypt(e.to_string()))?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            writer
                .write_all(&buf[..n])
                .await
                .map_err(|e| FilesError::Encrypt(e.to_string()))?;
            byte_size += n as u64;
        }
        writer
            .close()
            .await
            .map_err(|e| FilesError::Encrypt(e.to_string()))?;

        // Blobs hash plain — the stated §2.15 dedup exception.
        let hash = ContentHash {
            alg: HashAlg::Sha256,
            digest: hasher.finalize().into(),
        };

        if self.has(&hash).await? {
            self.store.delete(&staging).await.ok();
            return Ok(hash); // dedup: already stored, nothing to do
        }
        match self
            .keyring
            .register(&hash, &identity, meta.created_at)
            .await
        {
            Ok(()) => {}
            Err(KeyringError::AlreadyRegistered(_)) => {
                // A concurrent put of the same content won; its bytes
                // are the ones the address names — but only once they
                // are actually there. Claiming success against a bare
                // key row would be silent loss (the row may be a
                // crashed put's orphan; `sweep` reclaims those).
                self.store.delete(&staging).await.ok();
                return if self.has(&hash).await? {
                    Ok(hash)
                } else {
                    Err(FilesError::PutInFlight(hash))
                };
            }
            Err(e) => {
                self.store.delete(&staging).await.ok();
                return Err(e.into());
            }
        }
        // Registered as the address's writer: from here every failure
        // must roll the key row back — a registration without a
        // manifest poisons the address (`put` would claim success while
        // `get` finds nothing, forever).
        let commit: Result<(), FilesError> = async {
            // Server-side move to the content address.
            self.store.copy(&staging, &Self::blob_path(&hash)).await?;
            self.write_manifest(
                &hash,
                &ManifestRecord {
                    hash: hash.to_string(),
                    byte_size,
                    content_type: meta.content_type,
                    created_at_unix_secs: unix(meta.created_at),
                    scan: None,
                },
            )
            .await
        }
        .await;
        self.store.delete(&staging).await.ok();
        if let Err(error) = commit {
            // Best-effort rollback; whatever it misses, `sweep`'s
            // orphaned-row pass reclaims after the grace window.
            self.keyring.shred(&hash).await.ok();
            self.store.delete(&Self::blob_path(&hash)).await.ok();
            return Err(error);
        }
        Ok(hash)
    }

    async fn get(&self, hash: &ContentHash) -> Result<impl AsyncRead + Send + Unpin, FilesError> {
        self.decrypt_stream(hash, None).await
    }

    async fn get_range(
        &self,
        hash: &ContentHash,
        range: Range<u64>,
    ) -> Result<impl AsyncRead + Send + Unpin, FilesError> {
        self.decrypt_stream(hash, Some(range)).await
    }

    async fn has(&self, hash: &ContentHash) -> Result<bool, FilesError> {
        match self.store.head(&Self::manifest_path(hash)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
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
        for path in [Self::manifest_path(hash), Self::blob_path(hash)] {
            match self.store.delete(&path).await {
                Ok(()) => {}
                Err(object_store::Error::NotFound { .. }) => {}
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

        // Abandoned in-flight puts: staging objects older than the
        // grace window can only be crashes.
        let mut staged = self.store.list(Some(&ObjPath::from("tmp")));
        while let Some(entry) = staged.next().await {
            let entry = entry?;
            let modified: SystemTime = entry.last_modified.into();
            if now.duration_since(modified).unwrap_or(Duration::ZERO) >= grace {
                self.store.delete(&entry.location).await.ok();
                report.tmp_removed += 1;
            }
        }

        let mut candidates = Vec::new();
        let mut manifests = self.store.list(Some(&ObjPath::from("manifest")));
        while let Some(entry) = manifests.next().await {
            let entry = entry?;
            let Some(stem) = entry
                .location
                .filename()
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            let Ok(hash) = stem.replacen('-', ":", 1).parse::<ContentHash>() else {
                return Err(FilesError::Corrupt(format!(
                    "unparseable manifest name {stem}"
                )));
            };
            candidates.push(hash);
        }
        for hash in candidates {
            if live.contains(&hash) {
                report.kept_live += 1;
                continue;
            }
            let info = self.stat(&hash).await?;
            let age_of = now
                .duration_since(info.created_at)
                .unwrap_or(Duration::ZERO);
            if age_of < grace {
                report.kept_young += 1; // §13.6: young blobs are protected
                continue;
            }
            self.delete(&hash).await?;
            report.deleted.push(hash);
        }

        // Crash-orphaned key rows: a registration older than the grace
        // window with no manifest behind it is a put that died between
        // register and commit. Shred it so the address heals (a bare
        // row makes every re-put of that content fail with
        // `PutInFlight`), and drop any ciphertext the crash left at
        // the content address.
        if let Some(cutoff) = now.checked_sub(grace) {
            for hash in self.keyring.registered_before(cutoff).await? {
                if self.has(&hash).await? {
                    continue; // a stored blob's key row — keep
                }
                self.keyring.shred(&hash).await?;
                self.store.delete(&Self::blob_path(&hash)).await.ok();
                report.keys_removed += 1;
            }
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
        PutMeta {
            content_type: "application/pdf".into(),
            created_at: now(),
        }
    }

    async fn store(dir: &tempfile::TempDir) -> ObjectBlobStore<MemoryKeyring> {
        ObjectBlobStore::local(dir.path(), MemoryKeyring::default()).unwrap()
    }

    async fn read_all(store: &ObjectBlobStore<MemoryKeyring>, hash: &ContentHash) -> Vec<u8> {
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

        // On disk (local backend = real files): age ciphertext, never
        // the plaintext.
        let on_disk = std::fs::read(dir.path().join("blobs").join(stem(&hash))).unwrap();
        assert!(on_disk.starts_with(b"age-encryption.org/v1"));
        assert!(!on_disk.windows(plain.len()).any(|w| w == plain));

        let info = store.stat(&hash).await.unwrap();
        assert_eq!(info.byte_size, plain.len() as u64);
        assert_eq!(info.content_type, "application/pdf");
        assert_eq!(info.created_at, now());
    }

    #[tokio::test]
    async fn get_range_across_chunk_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        // > 4 STREAM chunks (64 KiB each), deterministic content.
        let plain: Vec<u8> = (0..300_000u32)
            .map(|i| (i.wrapping_mul(31) % 251) as u8)
            .collect();
        let hash = store.put(meta(), plain.as_slice()).await.unwrap();

        let mut reader = store.get_range(&hash, 200_123..201_123).await.unwrap();
        let mut window = Vec::new();
        reader.read_to_end(&mut window).await.unwrap();
        assert_eq!(window, plain[200_123..201_123]);

        let mut reader = store.get_range(&hash, 7..23).await.unwrap();
        let mut small = Vec::new();
        reader.read_to_end(&mut small).await.unwrap();
        assert_eq!(small, plain[7..23]);

        // Past-the-end truncates, HTTP-style.
        let mut reader = store.get_range(&hash, 299_990..400_000).await.unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).await.unwrap();
        assert_eq!(tail, plain[299_990..]);
    }

    #[tokio::test]
    async fn provider_swap_is_a_constructor() {
        // The same pipeline over the in-memory backend: what an
        // S3-compatible provider sees, minus the network.
        let store = ObjectBlobStore::memory(MemoryKeyring::default());
        let plain: Vec<u8> = (0..200_000u32)
            .map(|i| (i.wrapping_mul(17) % 249) as u8)
            .collect();
        let hash = store.put(meta(), plain.as_slice()).await.unwrap();

        let mut reader = store.get(&hash).await.unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, plain);

        let mut reader = store.get_range(&hash, 100_000..100_100).await.unwrap();
        let mut window = Vec::new();
        reader.read_to_end(&mut window).await.unwrap();
        assert_eq!(window, plain[100_000..100_100]);

        store.delete(&hash).await.unwrap();
        assert!(!store.has(&hash).await.unwrap());
    }

    #[tokio::test]
    async fn put_is_idempotent_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let a = store.put(meta(), &b"same bytes"[..]).await.unwrap();
        let b = store.put(meta(), &b"same bytes"[..]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(store.keyring.len(), 1);
        assert_eq!(
            std::fs::read_dir(dir.path().join("blobs")).unwrap().count(),
            1
        );
        // The losing put's staging object was cleaned up.
        let staged: Vec<_> = std::fs::read_dir(dir.path().join("tmp"))
            .map(|d| d.filter_map(Result::ok).collect())
            .unwrap_or_default();
        assert!(staged.is_empty());
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
        assert!(matches!(
            store.get(&hash).await.err(),
            Some(FilesError::NotFound(_))
        ));
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
        let old = PutMeta {
            content_type: "text/plain".into(),
            created_at: now(),
        };
        let referenced = store.put(old.clone(), &b"referenced"[..]).await.unwrap();
        let orphaned = store.put(old, &b"orphaned"[..]).await.unwrap();
        let young_meta = PutMeta {
            content_type: "text/plain".into(),
            created_at: now() + Duration::from_secs(3000),
        };
        let young = store.put(young_meta, &b"just uploaded"[..]).await.unwrap();

        let live = BTreeSet::from([referenced]);
        let at = now() + Duration::from_secs(3600);
        let report = store
            .sweep(&live, Duration::from_secs(1800), at)
            .await
            .unwrap();

        assert_eq!(report.deleted, vec![orphaned]);
        assert_eq!(report.kept_live, 1);
        assert_eq!(report.kept_young, 1);
        assert!(store.has(&referenced).await.unwrap());
        assert!(store.has(&young).await.unwrap());
        assert!(!store.has(&orphaned).await.unwrap());
        assert_eq!(store.keyring.len(), 2);
    }

    #[tokio::test]
    async fn sweep_removes_abandoned_staging() {
        let store = ObjectBlobStore::memory(MemoryKeyring::default());
        store
            .store
            .put(
                &ObjPath::from("tmp/put-999-0"),
                Bytes::from_static(b"crashed mid-put").into(),
            )
            .await
            .unwrap();

        let far_future = SystemTime::now() + Duration::from_secs(7200);
        let report = store
            .sweep(&BTreeSet::new(), Duration::from_secs(1800), far_future)
            .await
            .unwrap();
        assert_eq!(report.tmp_removed, 1);
    }

    /// The audit finding (2026-08-20): a key row without a manifest —
    /// a put that died between register and commit — must never make
    /// `put` claim success while `get` finds nothing. It fails loudly,
    /// young rows are protected, and sweep heals the address.
    #[tokio::test]
    async fn a_bare_key_row_fails_loudly_and_sweep_heals_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).await;
        let plain = b"unlucky content";
        let hash = ContentHash {
            alg: HashAlg::Sha256,
            digest: Sha256::digest(plain).into(),
        };
        // Simulate the crash: the registration exists, nothing else.
        store
            .keyring
            .register(&hash, &x25519::Identity::generate(), now())
            .await
            .unwrap();

        // Re-putting the same content is refused, never silently "ok".
        assert!(matches!(
            store.put(meta(), &plain[..]).await.err(),
            Some(FilesError::PutInFlight(h)) if h == hash
        ));

        // Within the grace window the row could still be an in-flight
        // put's — protected.
        let grace = Duration::from_secs(1800);
        let report = store
            .sweep(&BTreeSet::new(), grace, now() + Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(report.keys_removed, 0);

        // Past it, it can only be a crash: shredded, address healed.
        let report = store
            .sweep(&BTreeSet::new(), grace, now() + Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(report.keys_removed, 1);
        assert!(store.keyring.is_empty());
        let stored = store.put(meta(), &plain[..]).await.unwrap();
        assert_eq!(stored, hash);
        assert_eq!(read_all(&store, &hash).await, plain);
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
