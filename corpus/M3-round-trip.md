# M3 corpus round-trip run

Output of `cargo run --release -p m0 -- corpus/data/demarches.json --wire`
over the 2026-08-15 data.gouv.fr snapshot (see `M0-type-frequency.md`).
Exit criterion (§8 M3): the corpus in and out, byte-stable.

**Result: PASS.** All 42,723 schemas emitted as `revision` lines in one
history-mode stream (518 MB), read back, re-emitted **byte-identically**;
zero schema mismatches; every revision id recomputed from the *decoded*
schema equals the id on the wire — the canonical form is an identity
across the encode/decode boundary, not merely a deterministic encoder.

```
schemas emitted:            42723
distinct revision ids:      34302
stream bytes:           518191729
lines read back:            42724
byte-stable re-emit:          YES
schema mismatches:              0
revision id mismatches:         0

M3: PASS — the corpus round-trips byte-stably
```

Notes:

- ~30 s wall clock for 518 MB (write + read + re-write + compare).
- **34,302 distinct revision ids from 42,723 procedures**: 8,421
  procedures (19.7%) are structurally identical to another one.
  Content-addressing (§2.13) surfaces the corpus's real duplication rate
  as a side effect of convergence — the same schema hashes to the same
  id wherever it lives.
- Re-run 2026-08-18 after resolver declarations gained their anchor
  group (§10 Q17): +536 KB of `anchor` fields; distinct revision ids
  unchanged at 34,302.
- Records are not in this dataset; the record-side round trip (entry
  lines, chain adoption) is covered by `varve-wire`'s tests and property
  suites and awaits DN record data for a corpus run.
