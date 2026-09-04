//! `impl HostView` on Smithay -- the reference host.
//!
//! This crate answers the three questions the rest of the system cannot:
//! who owns a surface, whether a rect on it can actually be seen, and how to
//! put input into the seat so that focus, grabs and z-order stay correct by
//! construction rather than by imitation.
//!
//! Scope for M2 is two backends and no more: `--headless` (a virtual output;
//! also the CI rig) and `--nested` (runs inside an existing X11 or Wayland
//! session). Real DRM, modesetting and multi-output wait for `--seat`; they are
//! the part of compositor work that consumes schedule without proving anything.
//!
//! The signal worth building for is the disagreement between two feeds: surface
//! damage says *something changed on screen*, accessibility events say *what
//! changed semantically*. Damage arriving with no accompanying event is the
//! precise definition of a surface that renders without explaining itself, and
//! the only honest trigger for a vision fallback.

#![allow(
    dead_code,
    reason = "M0 skeleton: fixes the seam and the unsafe_code \
    posture for this crate before M2 gives it a body."
)]
