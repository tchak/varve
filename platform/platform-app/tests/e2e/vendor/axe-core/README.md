# axe-core, vendored

`axe.min.js` is the unmodified build of
[axe-core](https://github.com/dequelabs/axe-core) **4.13.0**
(npm `axe-core@4.13.0`, `package/axe.min.js`), licensed MPL-2.0
(`LICENSE` alongside). It is test-only: `tests/e2e/harness.rs`
injects it into each page under test and runs `axe.run` — the same
thing `@axe-core/playwright` does in the JS ecosystem, which has no
Rust counterpart.

Vendored rather than fetched so the e2e suite needs no network and
the rule set is pinned: a bump is a deliberate change (new rules
can fail pages) and replaces this file, the version above, and the
license if it changed.
