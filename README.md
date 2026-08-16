# Varve

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

`DESIGN.md` is the full design document and single source of truth,
including every decision's rationale and the open questions.

## Status

Pre-publish (`publish = false` everywhere; nothing on crates.io).

| milestone | state |
|---|---|
| **M0 — expressibility** | ✅ all 42,723 published DN procedures express and validate with **zero residue** (`corpus/M0-expressibility.md`) |
| **M1 — falsification** | machinery built (`varve-projection`, `varve-impact`); awaits historical revision data |
| **M2 — logic language** | not started; awaits the rule corpus |
| **M3 — wire round-trip** | not started |

## Layout

- `crates/varve-core` — ids, row paths, scalar primitives, canonical
  bytes and content addresses (JCS, SHA-256, salted commitments)
- `crates/varve-schema` — types, groups, nomenclatures, resolver
  declarations, validation, the cast table and type join
- `crates/varve-value` — cells, typed conformance, structural diff/patch
- `crates/varve-record` — the append-only record log: entries, fold,
  provenance, chain verification, snapshots, checkpoints, resolutions
- `crates/varve-projection` — records viewed through revisions they
  weren't written on; casts applied, lossiness reported
- `crates/varve-impact` — the impact report
- `tools/m0` — the corpus harness (oracle over the public DN dataset)
- `corpus/` — corpus analyses and results
- `DESIGN.md` — the design document

Everything below the storage tier is deterministic: no IO, no clock, no
async — timestamps and salts are inputs.

## Developing

Version control is [jj](https://github.com/jj-vcs/jj) (colocated git).

```sh
cargo test --workspace            # 65 tests, incl. property suites
cargo clippy --workspace --all-targets
cargo run --release -p m0 <path>  # M0 harness over the DN corpus
```

The corpus is public data:
[Descriptif des démarches publiées](https://www.data.gouv.fr/datasets/descriptif-des-demarches-publiees-sur-demarche-numerique-gouv-fr)
(data.gouv.fr, ~124 MB gzipped JSON).
