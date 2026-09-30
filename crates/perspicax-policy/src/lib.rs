//! Window-management decisions, as data.
//!
//! The compositor knows *how* to focus a surface, raise a window or start a
//! program. This crate decides *whether* and *which*: given what is under the
//! pointer and what holds the keyboard, which window should be focused and
//! which raised; given a chord, what it means.
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

pub use crate::{
    binds::{Action, Bindings, Chord, Mods},
    focus::{Change, Decision, Focus, FocusModel, cycle},
};

/// Keysyms, re-exported so a caller spells chords with the same type this
/// crate matches them against.
pub use xkeysym::Keysym;
