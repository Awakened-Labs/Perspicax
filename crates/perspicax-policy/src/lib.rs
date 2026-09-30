//! Window-management decisions, as data.
//!
//! The compositor knows *how* to focus a surface, raise a window or start a
//! program. This crate decides *whether*, *which* and *where*. Given what is
//! under the pointer and what holds the keyboard, it decides which window
//! should be focused and which raised. Given a chord or a modifier-drag, it
//! decides what that means. Given a drag, an output or a maximized window, it
//! decides where a window goes and how big it is.
//!
//! It imports no Smithay and does no I/O, for the same reason
//! `perspicax-index` doesn't. A decision that can be written as a function of
//! plain values can be tested as one, without a seat, a GPU or a Wayland
//! client. Occlusion is tested that way in `perspicax-index/src/host.rs`, and
//! these decisions are tested the same way. Everything is generic over the
//! window handle, so the compositor passes its own ids and the tests pass
//! integers.
//!
//! Config (`perspicax-config`, W1 slice 5) will build these values from
//! `config.toml` and the `classic`/`minimal` profiles. Until then the
//! compositor uses [`Focus::default`] and [`Bindings::classic`].

mod binds;
mod focus;
mod geometry;

pub use crate::{
    binds::{Action, Bindings, Button, Chord, Drag, Mods, Towards},
    focus::{ACTIVATION_WINDOW, Change, Decision, Focus, FocusModel, cycle, grants_activation},
    geometry::{Edges, Rect, anchor, carry, edges_near, neighbour, place, resize, unmaximized_at},
};

/// Keysyms, re-exported so a caller spells chords with the same type this
/// crate matches them against.
pub use xkeysym::Keysym;
