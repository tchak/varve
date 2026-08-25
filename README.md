# Varve

[![CI](https://github.com/tchak/varve/actions/workflows/ci.yml/badge.svg)](https://github.com/tchak/varve/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/tchak/varve/graph/badge.svg)](https://codecov.io/gh/tchak/varve)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 2024](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](Cargo.toml)
![Status: pre-publication, 0.1.0 placeholder](https://img.shields.io/badge/status-pre--publication-lightgrey.svg)

A generic, embeddable Rust kernel for **versioned, multi-party case
files**, extracted from ~10 years of building a large French
public-service platform (administrations publish *procedures* — schemas —
and citizens submit *dossiers* — records; millions of them).

> A *varve* is an annual layer of lake sediment: an append-only sequence
> of timestamped layers whose value is that history can be read back from
> them.

## The thesis

The valuable extraction is **not the form builder** — good open-source
form builders exist. It is the versioning kernel underneath:

- **schema revisions applied to live records** — hundreds of thousands of
  them, with a per-column compatibility relation (Avro-style
  reader/writer resolution) instead of freeze-or-corrupt;
- **records as long-lived case files** — appended to by many actors over
  time (applicant, instructors, third parties, external data sources),
  as an append-only, hash-chained, tamper-evident log;
- **provenance as a kernel concept** — every cell knows whether it was
  entered by a human, derived from an authoritative source (with the
  retained payload), or overridden — the *données déclaratives* vs
  *référentiel authentique* distinction, mechanized;
- **an impact report** shown to an administration *before* it publishes
  a revision: what breaks, what loses information, and exactly which
  records fail — the artifact no form platform offers.

`design/kernel.md` is the full kernel design document and single source
of truth, including every decision's rationale and the open questions;
`design/platform.md` designs the platform above it and
`design/graphql.md` the public schema.

## Status

Pre-publish (`publish = false` everywhere; nothing on crates.io).

| milestone | state |
|---|---|
| **M0 — expressibility** | ✅ all 42,723 published DN procedures express and validate with **zero residue** (`corpus/M0-expressibility.md`) |
| **M1 — falsification** | machinery built (`varve-projection`, `varve-impact`); awaits historical revision data |
| **M2 — logic language** | predicate core built (`varve-logic`); awaits the rule corpus for falsification |
| **M3 — wire round-trip** | ✅ all 42,723 schemas round-trip byte-stably through `varve-wire` (`corpus/M3-round-trip.md`); record-side awaits DN data |

## Layout

- `crates/varve-core` — ids, row paths, scalar primitives, canonical
  bytes and content addresses (JCS, SHA-256, salted commitments)
- `crates/varve-schema` — types, groups, blocks (schema side),
  nomenclatures, resolver declarations, validation, the cast table and
  type join
- `crates/varve-value` — cells, typed conformance, structural diff/patch
- `crates/varve-logic` — the predicate language: AST and canonical form,
  typechecker, total evaluator, rule-graph acyclicity
- `crates/varve-projection` — records viewed through revisions they
  weren't written on; casts applied, lossiness reported
- `crates/varve-impact` — the impact report: change classification,
  resolver questions, broken rule references, record assessment
- `crates/varve-record` — the append-only record log: entries, fold,
  provenance, chain verification, snapshots, checkpoints, resolutions, scans
- `crates/varve-surface` — presentation and admissibility: reachability,
  requiredness, formats, write policy, block defaults
- `crates/varve-revision` — revision DAG and publication, block and
  nomenclature registries, three-way schema merge, aggregate revisions
- `crates/varve-bundle` — the export bundle (Tier 5): blob sidecar
  assembly/import, surfaces joined with their wire envelope
- `crates/varve-wire` — tagged JSONL: writer, reader, history and
  snapshot import
- `crates/varve-files` — Tier 5: content-addressed blob store for
  attachments and resolver payloads over `object_store` backends;
  encrypting implementations take a per-blob X25519 keyring
- `crates/varve-store` — Tier 5: async persistence traits for kernel
  objects (registries, surfaces, record logs) plus the in-memory
  reference implementation
- `crates/varve-service` — the choreography narrow waist:
  transactional sequences over the store traits (first operation:
  impact-gated publication)
- `platform/` — the platform above the kernel (`design/platform.md`):
  `platform-app` (the topcoat web app), `-server` (the binary),
  `-core` (platform domain: accounts, organizations, procedures, the
  authored tree), `-store` (the `varve-store` traits over PostgreSQL),
  `-graphql` (the public schema, executed in-process), `-client` (its
  typed transport-agnostic client), `-i18n` (the MessageFormat 2
  runtime over ICU4X)
- `tools/m0` — the corpus harness (oracle over the public DN dataset)
- `fuzz/` — cargo-fuzz targets (see below)
- `corpus/` — corpus analyses and results
- `design/` — the design documents: `kernel.md`, `platform.md`, `graphql.md`

Everything below the storage tier is deterministic: no IO, no clock, no
async — timestamps and salts are inputs.

## Developing

Version control is [jj](https://github.com/jj-vcs/jj) (colocated git).

```sh
cargo test --workspace              # 567 tests + doctests, incl. property suites
cargo clippy --workspace --all-targets
cargo fmt --all --check
topcoat fmt platform                # formats view! macro bodies, which rustfmt leaves alone
scripts/check-layering.sh           # DESIGN §13.5: no runtime/web/ORM crate below Tier 5
scripts/fetch-corpus.sh             # download the DN corpus (~124 MB gz)
cargo run --release -p m0           # M0 harness over the corpus
```

CI (`.github/workflows/ci.yml`) runs the same checks on every push
and PR — tests under [cargo-nextest](https://nexte.st) (one process
per test; prefer it locally too, plain `cargo test` can hide
cross-process races), `cargo doc` with warnings denied, the layering
guard, and a fuzz regression pass. Two more workflows: `fuzz.yml`
fuzzes each target for ten minutes weekly (or on demand, with a
chosen duration), merges new coverage into the seeds and opens a pull
request with them; `coverage.yml` measures coverage with
cargo-llvm-cov on every push to main and publishes it to
[Codecov](https://app.codecov.io/gh/tchak/varve).

### The platform app locally

The web app is a [topcoat](https://github.com/tokio-rs/topcoat) app
over PostgreSQL. One-time setup:

```sh
cargo install topcoat-cli           # the `topcoat` binary (dev server, fmt, assets)
createdb varve_platform_dev
cp .env.example .env                # then fill in COOKIE_KEY (openssl rand -base64 64)
```

Then:

```sh
topcoat dev -p platform-server
```

builds, bundles the assets (the Tailwind stylesheet), serves on
`127.0.0.1:3000`, and watches: on change it rebuilds, rebundles,
restarts, and open pages reload themselves. Pending migrations apply
at boot. Plain `cargo run -p platform-server` also works, minus
assets (pages come up unstyled) and reload.

### The DB-backed and browser tests

`cargo test --workspace` needs no setup: tests gated on a database or
a browser pass vacuously without one. To run them for real, the way
CI does:

```sh
createdb varve_platform_test
export VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test  # real env, not .env
cargo run -p platform-app --example install-browsers  # Playwright engines, pinned via Cargo.lock
cargo nextest run --workspace
```

The browser e2e suite (`platform/platform-app/tests/e2e/`) runs on
every installed Playwright engine — chromium, firefox, webkit — and
skips the absent ones.

Fuzz targets live in `fuzz/` (excluded from the workspace; needs
`cargo install cargo-fuzz` and a nightly toolchain):

```sh
cd fuzz
cargo +nightly fuzz run wire_read corpus/wire_read seeds/wire_read   # also: logic_canon, value_feature, record_entry
cargo +nightly fuzz run wire_read seeds/wire_read -- -runs=0         # regression only: each seed once
cargo +nightly fuzz run wire_read seeds/wire_read corpus/wire_read -- -merge=1   # fold new coverage into the seeds
```

`fuzz/seeds/<target>/` is the tracked regression corpus: a minimized
(`-merge=1`) set of inputs, each adding coverage, which CI replays with
`-runs=0` so a decoder change that starts rejecting, accepting or
crashing on a known input fails the build. The working corpora under
`fuzz/corpus/` (where a run writes what it finds) are gitignored; after
a local fuzzing session, or after a fix that changes what a decoder
accepts, merge them back into the seeds with the last command above —
the weekly workflow does the same and proposes the result as a PR.

The corpus is public data —
[Descriptif des démarches publiées](https://www.data.gouv.fr/datasets/descriptif-des-demarches-publiees-sur-demarche-numerique-gouv-fr)
(data.gouv.fr) — fetched into the gitignored `corpus/data/`. Snapshots
are dated; numbers in `corpus/*.md` come from the 2026-08-15 snapshot.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
