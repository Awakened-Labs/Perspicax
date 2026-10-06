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
//! (`tabs`); and which programs may use the protocols that reach past their
//! own windows (`access`); and whether a display tool's request for the
//! monitors can be carried out (`heads`); and whether a press of the Logo key
//! was a tap (`tap`); and when to start the desktop shell again after it
//! stops (`restart`). `perspicax-config`
//! builds their settings from `config.toml` and the `classic`/`minimal`
//! profiles.

mod access;
mod binds;
mod flip;
mod focus;
mod frame;
mod geometry;
mod heads;
mod layout;
mod restart;
mod snap;
mod tabs;
mod tap;
mod theme;
mod workspace;

pub use crate::{
    access::{Access, Program, Protocol, Rule},
    binds::{Action, Bindings, Button, Chord, Drag, Mods, Towards, candidates},
    flip::{EdgeDwell, Flipping, NOTCH_PIXELS, Notches, arrival, edge_at},
    focus::{ACTIVATION_WINDOW, Change, Decision, Focus, FocusModel, cycle, grants_activation},
    frame::{
        Colour, Decorations, FrameButton, GRIP, Insets, Look, Part, Press, buttons, buttons_in,
        fit, frame_rects, inset, is_double, outset, part_at, tab_rects, titlebar,
    },
    geometry::{
        DRAG_THRESHOLD, Edges, Rect, anchor, carry, dragged, edges_near, neighbour, place, resize,
        unmaximized_at,
    },
    heads::{Head, HeadChange, HeadMode, ModeChoice, Rejection, check_heads},
    layout::{Place, Screen, Side, arrange, intersects, overlapping, rescue},
    restart::{EX_CONFIG, Ended, Restart, Restarts},
    snap::{Snapping, Zone, keyed, zone},
    tabs::Groups,
    tap::{LogoTap, is_logo},
    theme::{
        Appearance, Builtin, ColorScheme, Contrast, Family, Font, Palette, Rgba, Role, Theme,
        follows,
    },
    workspace::{Cell, Direction, Grid, GroupView, Home, Mode, Shape, WorkspaceView, Workspaces},
};

/// Keysyms, re-exported so a caller spells chords with the same type this
/// crate matches them against.
pub use xkeysym::Keysym;
