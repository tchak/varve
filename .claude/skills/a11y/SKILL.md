---
name: a11y
description: Accessibility contract for the varve platform web app (PLATFORM.md P.1.5 — RGAA 4.1 / WCAG 2.2 AA). ALWAYS load before writing or reviewing any page, component, form, or e2e test under platform/ — it states which test level owns which accessibility proof, the markup checklist every page meets, how the automated baseline (router-level HTML lint + axe-core in e2e) is run and extended, and what stays manual. Also load when a lint or axe failure needs fixing, or when touching vendored topcoat-ui components.
---

# Accessibility in `platform/` — the contract

Varve's platform succeeds a French public-service site: accessibility is
a legal duty (RGAA 4.1 — loi 2005-102, décret 2019-768), not polish.
Target **RGAA 4.1 / WCAG 2.2 AA** on every page. Principle: PLATFORM.md
P.1.5; open items (contrast audit, manual audit, declaration, the
details-menu Escape gap): P.9 Q12.

## Who proves what (CLAUDE.md platform test policy)

| Level | Owns | Mechanism |
|---|---|---|
| Component tests (`src/components/*`, `#[cfg(test)]`) | aria wiring of *our* components: label↔control, `aria-describedby` + `aria-invalid` on error, names on icon-only controls | plain `#[test]` via `components::testing::render` |
| Router tests (`tests/app/`) | the **static baseline** over every HTML response | `harness::body_text` runs `a11y_baseline(html)` on every `text/html` body — automatic, never opt out |
| Browser e2e (`tests/e2e/a11y.rs`) | the **rule engine** and keyboard | `harness::check_axe(&page, label)` (vendored axe-core, WCAG 2.x A/AA + best-practice tags) on every page/state; Tab/Enter journeys with `to_be_focused` |

A failure at any level is fixed in the page or component — never
allowlisted, never asserted around. Automated checks catch roughly a
third of WCAG failures; they stop regressions, they are not the audit.

## The static baseline (`tests/app/harness.rs::a11y_baseline`)

Decidable from markup alone; one message per violation:

- `<html lang>` present (RGAA 8.3)
- exactly one `<main>` (9.2); exactly one `<h1>`; heading levels never
  skip (9.1) — a vendored `<h3>` card title under an `<h1>` needs an
  `<h2>` between, `sr-only` if the visible design has none (see
  `settings_shell`)
- every `<img>` has `alt` (1.1)
- every `input`/`select`/`textarea` (not hidden/submit/button/reset/
  image) is labelled: `<label for>`, wrapping `<label>`, `aria-label`,
  or `aria-labelledby` (11.1)
- every `button`, `a[href]`, `[role=button|link]` has an accessible
  name: text, `aria-label`, `aria-labelledby`, `img[alt]`, or
  `svg > title` (6.1, 7.1)
- `aria-describedby` / `aria-labelledby` ids exist; `aria-invalid=true`
  controls carry `aria-describedby` (11.10)

Extend it when a new class of markup defect shows up that markup alone
can decide; add the rule *and* its self-test in the `a11y_baseline_tests`
module. Keep browser-only rules (contrast, focus, computed roles) out —
that is axe's job.

## axe in e2e (`tests/e2e/harness.rs::check_axe`)

- Vendored, pinned: `tests/e2e/vendor/axe-core/` (`axe.min.js`,
  `LICENSE` MPL-2.0, `README.md` with the version). Bumping is a
  deliberate change: new rules can fail pages.
- Injected with `page.add_script_tag(content)`, then
  `page.evaluate("() => axe.run(document, {runOnly: {type: 'tag', ...}})
  .then(r => r.violations)")`, deserialized into `AxeViolation`. The
  failure lists rule id, impact, help URL, each node's selector and
  HTML, and axe's `failureSummary`.
- The harness serves the app **styled**: it bundles the Tailwind
  stylesheet out of the test binary itself (`assets()`), same scan as
  `topcoat asset bundle`, so bundle and binary come from one build.
  Never run axe against an unstyled page — contrast and target-size
  results are noise there (that is how the first run produced bogus
  `target-size` hits on the header).
- `tests/e2e/a11y.rs` is the sweep: every page, both locales, the
  states a journey reaches (validation errors, the open account menu).
  **A new page joins the sweep; a page left out is a page unchecked.**
  Other subjects may `check_axe` a mid-flow state the sweep cannot
  reach, never as a substitute for it.
- Run: `VARVE_TEST_DATABASE_URL=... cargo test -p platform-app --test
  e2e a11y::` (all installed engines; WebKit skips signed-in states —
  it refuses the Secure cookie over loopback http, encoded explicitly).

## Keyboard journeys

- Assert focus with `expect(page.locator(locator!("header a[href='/']"))).to_be_focused()`
  — **single page-level CSS selector only**: playwright-rs 0.16 hands
  the selector to `querySelectorAll`, so role locators and `>>` chains
  error out there. Use role locators for visibility/name assertions.
- Drive with `page.keyboard().press("Tab"|"Enter"|"Escape", None)`;
  never synthesize key events via `evaluate`.
- Known gap (P.9 Q12 d): the account menu is a `<details>`; Escape does
  not close it. The journey asserts what holds; do not weaken it.

## Markup checklist for a new page or component

1. Landmarks: `<header>/<nav>`, one `<main>`, `<footer>` if any; one
   `<h1>` (`page_title`), headings in order.
2. Every form control through `field` (label, `aria-describedby`,
   `aria-invalid` come for free) or a `label` + explicit `for`/`id`.
   Errors are text next to the control, referenced from it — never
   colour alone.
3. Flash/alerts: `role="alert"` for errors that appear after an action;
   `role="status"` for neutral confirmation.
4. Icon-only controls get `aria-label` (see the account-menu trigger);
   decorative icons are `aria-hidden`.
5. Links navigate, buttons act: a sign-out is a `<form method=post>`
   with a button, not a link.
6. Touch targets ≥ 24×24 CSS px or spaced (WCAG 2.5.8): button sizes
   `Sm`+ are fine; bare inline text links inside toolbars are not.
7. Focus is visible (`focus-visible` ring from the theme — do not
   remove outlines); after a redirect the page starts at its `<h1>`.
8. `lang` follows the resolved locale; French strings keep U+00A0/
   U+202F before `:` `;` `?` `!` — and tests substring-match around them.
9. Respect `prefers-reduced-motion` for any animation; contrast comes
   from theme tokens only (overrides are a recorded decision).
10. Then: router test fetches the page (baseline runs itself); add the
    page to the e2e sweep; if it has an error or open state, add that
    state too.

## Vendored components (`components.toml`, `tests/registry_sync.rs`)

Registry components are pinned byte-for-byte. An accessibility defect
inside one is an **upstream fix** (PR to topcoat-ui), or a wrapper in
our own component — never an in-place edit. If a wrapper is the fix,
note the upstream issue in the wrapper's docs.

## Manual, not automated (P.9 Q12)

Screen readers (VoiceOver, NVDA), 200 % zoom and reflow, the theme's
contrast audit in both schemes, and DINUM's **Ara** RGAA grid before
any accessibility declaration. Schedule the first pass no later than
the first reviewer-facing release.
