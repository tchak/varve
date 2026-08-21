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
pub mod badge;
pub mod button;
pub mod card;
pub mod input;
pub mod label;
pub mod select;
pub mod tabs;

// Ours (not in the registry; listed in `tests/registry_sync.rs`).
pub mod field;
pub mod page_title;
pub mod site_header;

/// Shared machinery for the component tests (the policy's level 1,
/// CLAUDE.md § Platform test policy): building a component's view is
/// an async burst (the root `view!` awaits its component futures),
/// but pure presentational components never wait on external events,
/// so a noop-waker poll loop — the same pattern topcoat-view's own
/// unit tests use — drives it to completion in a plain `#[test]`.
#[cfg(test)]
pub(crate) mod testing {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use topcoat::{
        Result,
        context::{Cx, CxTestBuilder},
    };

    /// Builds the view `build` returns against a bare test `Cx` and
    /// renders it to HTML.
    pub(crate) fn render(build: impl AsyncFnOnce(&Cx) -> Result) -> String {
        let cx = CxTestBuilder::new().build();
        let view = block_on(build(&cx)).expect("build the view");
        view.render(&cx)
    }

    /// Drives `fut` to completion on the current thread. The
    /// components under test never wait on external events, so
    /// polling in a tight loop is sufficient.
    fn block_on<F: Future>(fut: F) -> F::Output {
        let mut fut = pin!(fut);
        let mut task = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(output) = fut.as_mut().poll(&mut task) {
                return output;
            }
        }
    }
}
