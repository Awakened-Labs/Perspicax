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
//! The decisions, by module: focus and what a chord means (`focus`, `binds`);
//! where a window goes and how big (`geometry`); where each monitor sits
//! (`layout`); which workspace shows which windows (`workspace`); changing
//! workspace with the pointer (`flip`); snapping to halves and quarters
//! (`snap`); the frame drawn around a window (`frame`); and tab groups
//! (`tabs`). `perspicax-config`
//! builds their settings from `config.toml` and the `classic`/`minimal`
//! profiles.

mod binds;
mod flip;
mod focus;
mod frame;
mod geometry;
mod layout;
mod snap;
mod tabs;
mod workspace;

pub use crate::{
    binds::{Action, Bindings, Button, Chord, Drag, Mods, Towards},
    flip::{EdgeDwell, Flipping, NOTCH_PIXELS, Notches, arrival, edge_at},
    focus::{ACTIVATION_WINDOW, Change, Decision, Focus, FocusModel, cycle, grants_activation},
    frame::{
        Colour, Decorations, FrameButton, GRIP, Insets, Look, Part, Press, buttons, buttons_in,
        fit, frame_rects, inset, is_double, outset, part_at, tab_rects, titlebar,
    },
    geometry::{Edges, Rect, anchor, carry, edges_near, neighbour, place, resize, unmaximized_at},
    layout::{Place, Screen, Side, arrange, intersects, overlapping, rescue},
    snap::{Snapping, Zone, keyed, zone},
    tabs::Groups,
    workspace::{Cell, Direction, Grid, Home, Mode, Shape, Workspaces},
};

/// Keysyms, re-exported so a caller spells chords with the same type this
/// crate matches them against.
pub use xkeysym::Keysym;
