//! The agent interface -- an MCP server over stdio (M3).
//!
//! Tools: `window_list`, `observe`, `resolve`, `act`, `subscribe`,
//! `capability_grant` / `capability_list`, and `screenshot` -- the last
//! documented in its own description as the admission of defeat for one
//! surface, so that a model reading the tool list can tell the fallback from
//! the mechanism.
//!
//! Two design rules survive from the plan:
//!
//! - **Receipts, not booleans.** An act returns what happened: when it was
//!   dispatched, which node it resolved to, focus before and after, and whether
//!   damage followed within the window. An agent that gets `true` back has
//!   learned nothing about whether the application noticed.
//! - **The gate is not here.** [`check_actable`] lives in `wm-index` so that a
//!   future host speaking some other protocol cannot route around it. This
//!   crate adds the capability check on top of that, never instead of it.
//!
//! [`check_actable`]: wm_index::check_actable

#![allow(
    dead_code,
    reason = "M0 skeleton: fixes the seam before M3 gives it a body."
)]
