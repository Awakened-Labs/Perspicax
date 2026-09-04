//! `impl Ingest` over the AT-SPI2 accessibility bus.
//!
//! This is the slow path, and it ships first on purpose. It works against stock
//! GTK and Qt with no patched toolkit and no cooperation from the application,
//! which means the whole pipeline above it -- stable ids, selectors, the
//! compositor join, refusals, receipts -- can be proven against real programs
//! before any of the interesting transport work begins.
//!
//! Two constraints on whatever fills this in (M1):
//!
//! - **Never poll a tree.** A full AT-SPI read costs a D-Bus round trip per
//!   property and is measured in seconds. Bulk-read with `Collection.GetMatches`
//!   and invalidate from `object:children-changed`, `object:state-changed`,
//!   `object:text-changed` and `object:property-change`.
//! - **Never trust its coordinates as global.** A Wayland client cannot know
//!   its own position on screen, so `Component.GetExtents` is window-relative
//!   at best. Only a [`HostView`] turns bounds into anything global.
//!
//! [`HostView`]: wm_index::HostView

#![allow(
    dead_code,
    reason = "M0 skeleton: the crate exists to fix its seam \
    in the dependency graph -- it depends on wm-index and wm-node and nothing \
    Wayland-shaped -- before M1 gives it a body."
)]
