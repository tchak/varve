//! The **raw controls**: the vendored components' look as
//! `StaticClass` consts, for the inputs and selects the editor writes
//! as plain elements rather than component calls.
//!
//! Why they are not just `components::input` and friends: those take
//! their attributes as an [`Attributes`](topcoat::view::Attributes)
//! prop, and a runtime handler cannot travel that way — the closure's
//! captures would not outlive the call that builds the map. Every
//! control the editor autosaves therefore carries its own `@change`
//! on a plain `<input>` / `<select>` / `<textarea>`, and borrows the
//! registry component's classes from here.
//!
//! Kept in step by hand with `components::input`,
//! `components::select` and `components::switch`; `tests/registry_sync.rs`
//! pins those to the registry, so a drift there is a deliberate act
//! that should be mirrored here.

use topcoat::view::{StaticClass, class};

/// The vendored input's look, for the controls that carry runtime
/// handlers: a handler cannot travel through `attributes!` into a
/// component (its captures would not outlive the call), so these are
/// plain elements. Kept in step with `components::input::INPUT` and
/// `components::select::SELECT`.
pub(in crate::pages) const INPUT: StaticClass = class!(
    "h-9 w-full min-w-0 rounded-lg border border-border bg-background px-3 \
     text-sm shadow-xs transition-colors outline-none \
     placeholder:text-muted-foreground \
     focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 \
     focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-50",
);

pub(in crate::pages) const TEXTAREA: StaticClass = class!(
    "min-h-20 w-full min-w-0 rounded-lg border border-border bg-background px-3 py-2 \
     text-sm shadow-xs transition-colors outline-none \
     placeholder:text-muted-foreground \
     focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 \
     focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-50",
);

/// The vendored switch's look for the required toggle, which carries
/// a runtime handler (a handler cannot travel through `attributes!`
/// into a component). Kept in step with `components::switch`.
pub(in crate::pages) const SWITCH_TRACK: StaticClass = class!(
    "peer h-4.5 w-8 shrink-0 appearance-none rounded-full \
     bg-foreground/20 shadow-xs transition-colors outline-none checked:bg-primary \
     focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 \
     focus-visible:ring-offset-background disabled:pointer-events-none",
);

pub(in crate::pages) const SWITCH_THUMB: StaticClass = class!(
    "pointer-events-none absolute top-1/2 left-0.5 size-3.5 -translate-y-1/2 \
     rounded-full bg-background shadow-xs transition-transform peer-checked:translate-x-3.5",
);

pub(in crate::pages) const SELECT: StaticClass = class!(
    "h-9 w-full appearance-none items-center rounded-lg border border-border \
     bg-background pr-8 pl-3 text-left text-sm shadow-xs transition-colors outline-none \
     focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 \
     focus-visible:ring-offset-background disabled:pointer-events-none",
);
