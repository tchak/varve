//! The UI components: the themeable pieces every page is composed
//! from.
//!
//! Two kinds live side by side in `src/components/`:
//!
//! - **Vendored** from the built-in topcoat-ui registry by
//!   `topcoat ui add` (recorded in `components.toml`; refresh with
//!   `topcoat ui add <name> --overwrite`). Theirs to restyle, but
//!   `tests/registry_sync.rs` pins them byte-for-byte to the registry
//!   so drift is a deliberate, test-acknowledged act.
//! - **Ours** ([`field`], [`page_title`], [`site_header`]): created
//!   here following the registry components' conventions (`attrs`
//!   forwarding with `class` merge, `StaticClass` consts, display
//!   text as props — never message ids or literals).

// Vendored from the topcoat-ui registry (managed by `topcoat ui`).
pub mod alert;
pub mod button;
pub mod card;
pub mod input;
pub mod label;

// Ours (not in the registry; listed in `tests/registry_sync.rs`).
pub mod field;
pub mod page_title;
pub mod site_header;
