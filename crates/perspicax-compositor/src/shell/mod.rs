//! Window management: where windows go, what size they are, and the
//! interactive gestures that change both.
//!
//! The decisions come from `perspicax-policy` (placement, resize arithmetic,
//! where a dragged-out maximized window lands). This module carries them out
//! in xdg-shell terms: pending state, configures, and the grab that owns the
//! pointer while a window is being moved or resized.
//!
//! # Only with a person at the seat
//!
//! Headless keeps the M2 contract: a window stays where the cascade put it, at
//! the size it chose, and `maximize` and `fullscreen` requests are answered with
//! the plain configure Smithay sends by default. The occlusion tests are
//! written against those deterministic rectangles, and a headless client
//! reorganising its own geometry mid-test would be measuring this module
//! instead of the index. So every entry point that would move or resize a
//! window checks [`crate::backend::Running::has_person`] first.

mod actions;
mod grabs;
mod workspaces;

use std::cell::RefCell;

use perspicax_node::SurfaceId;
use perspicax_policy::{Edges, Rect, Towards, carry, neighbour, place, unmaximized_at};
use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
        find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat,
        pointer::{Focus, GrabStartData},
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    },
    utils::{Logical, Point, Rectangle, Serial, Size},
    wayland::{
        seat::WaylandFocus as _,
        shell::xdg::{PopupSurface, ToplevelSurface},
    },
};

use crate::state::Compositor;

pub(crate) use grabs::{MoveGrab, ResizeGrab};

/// What the compositor remembers about one window's placement, kept in the
/// window's own user data so it lives and dies with the window.
#[derive(Debug, Default)]
pub(crate) struct Placement {
    /// Where the window was, at what size, before it was maximized or made
    /// fullscreen, so unmaximizing can put it back.
    pub(crate) restore: Option<Rectangle<i32, Logical>>,
    /// Where it was when it was parked: minimized, or hidden with its
    /// workspace. A parked window is unmapped from the space, and this is how
    /// it gets its place back.
    pub(crate) parked: Option<Point<i32, Logical>>,
    /// The person minimized it. It stays off screen whichever workspace is
    /// showing, until it is restored.
    pub(crate) minimized: bool,
    /// A resize in progress, or finished and waiting for the client's last
    /// commit. See [`Compositor::settle_resize`].
    pub(crate) resize: Option<Resize>,
}

/// A resize, as the grab started it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Resize {
    pub(crate) start: Rect,
    pub(crate) edges: Edges,
    /// The pointer was released: the next commit is the last one this resize
    /// cares about.
    pub(crate) released: bool,
}

/// This window's placement record, created on first use.
pub(crate) fn placement<T>(window: &Window, f: impl FnOnce(&mut Placement) -> T) -> T {
    window
        .user_data()
        .insert_if_missing(|| RefCell::new(Placement::default()));
    let cell = window
        .user_data()
        .get::<RefCell<Placement>>()
        .expect("inserted on the line above");
    f(&mut cell.borrow_mut())
}

/// Smithay's rectangle as policy's.
pub(crate) fn rect(r: Rectangle<i32, Logical>) -> Rect {
    Rect::new(r.loc.x, r.loc.y, r.size.w, r.size.h)
}

impl Compositor {
    /// Where the next new window goes: cascaded on the output under the
    /// pointer, so a window opens on the monitor the person is looking at.
    ///
    /// Headless, the pointer starts at the origin and there is one output
    /// there, so this is the same cascade from `(0, 0)` that M2 placed
    /// windows with.
    pub(crate) fn place_new(&mut self) -> Point<i32, Logical> {
        // Inside the part of the output panels leave free, so a new window
        // never opens under a panel. Headless has no panels, so this is the
        // whole virtual output, as it always was.
        let area = self
            .output_under_pointer()
            .and_then(|output| self.usable_area(&output))
            .unwrap_or_default();
        let at = place(self.placed, rect(area));
        self.placed = self.placed.wrapping_add(1);
        at.into()
    }

    /// Where a window is and how big, in the global space: where it was
    /// mapped, at the size the client declared for it.
    ///
    /// Not `Space::element_geometry`, for the reason `publish_facts` gives:
    /// Smithay sizes a window from buffers only the seat backend records, so
    /// headless every window is `0x0` there, and nothing could tell which
    /// output one is on. An X11 window's geometry comes from the X server, and
    /// a client that declared nothing falls back to the buffer's.
    pub(crate) fn extent(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let location = self.space.element_location(window)?;
        Some(Rectangle::new(location, extent_size(window)))
    }

    /// The output a window is mostly on, or the one under the pointer.
    pub(crate) fn output_of(&self, window: &Window) -> Option<Output> {
        let bounds = self.extent(window);
        let overlap = |output: &&Output| {
            let (Some(bounds), Some(area)) = (bounds, self.space.output_geometry(output)) else {
                return 0;
            };
            bounds.intersection(area).map_or(0, |shared| {
                i64::from(shared.size.w) * i64::from(shared.size.h)
            })
        };
        self.space
            .outputs()
            .filter(|output| overlap(output) > 0)
            .max_by_key(overlap)
            .or_else(|| {
                self.pointer_location()
                    .and_then(|at| self.space.output_under(at).next())
            })
            .or_else(|| self.space.outputs().next())
            .cloned()
    }

    /// Fill an output with a window, remembering where it was.
    ///
    /// `state` is `Maximized` or `Fullscreen`. They differ in what the client
    /// draws (a fullscreen window drops its own decorations) and in what they
    /// fill: a maximized window stops at the panels' exclusive zones, a
    /// fullscreen one covers them.
    pub(crate) fn fill(
        &mut self,
        surface: &ToplevelSurface,
        state: xdg_toplevel::State,
        on: Option<&WlOutput>,
    ) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            surface.send_configure();
            return;
        };
        let output = on
            .and_then(Output::from_resource)
            .or_else(|| self.output_of(&window));
        // Maximized fills what the panels leave free; fullscreen covers the
        // panels too, which is the difference between the two.
        let area = output.and_then(|output| {
            if state == xdg_toplevel::State::Fullscreen {
                self.space.output_geometry(&output)
            } else {
                self.usable_area(&output)
            }
        });
        let Some(area) = area else {
            surface.send_configure();
            return;
        };
        if let Some(current) = self.space.element_geometry(&window) {
            placement(&window, |placement| {
                placement.restore.get_or_insert(current);
            });
        }
        surface.with_pending_state(|pending| {
            pending.states.set(state);
            pending.size = Some(area.size);
        });
        surface.send_configure();
        self.space.map_element(window, area.loc, false);
        self.publish_facts();
    }

    /// Undo [`Compositor::fill`]: back to where it was, at the size it was.
    ///
    /// `at` overrides where it goes back to, which is how a maximized window
    /// dragged by its titlebar comes out from under the pointer rather than
    /// jumping back to wherever it was before it was maximized.
    pub(crate) fn unfill(
        &mut self,
        surface: &ToplevelSurface,
        state: xdg_toplevel::State,
        at: Option<Point<i32, Logical>>,
    ) {
        let window = self.window_for(surface.wl_surface());
        let restore = window
            .as_ref()
            .and_then(|window| placement(window, |placement| placement.restore.take()));
        surface.with_pending_state(|pending| {
            pending.states.unset(state);
            pending.size = restore.map(|restore| restore.size);
        });
        surface.send_pending_configure();
        if let (Some(window), Some(restore)) = (window, restore) {
            self.space
                .map_element(window, at.unwrap_or(restore.loc), false);
            self.publish_facts();
        }
    }

    /// Whether this toplevel currently fills an output.
    pub(crate) fn is_filling(surface: &ToplevelSurface) -> bool {
        surface.with_pending_state(|pending| {
            pending.states.contains(xdg_toplevel::State::Maximized)
                || pending.states.contains(xdg_toplevel::State::Fullscreen)
        })
    }

    /// Send a window to the output beside the one it is on. A maximized or
    /// fullscreen window fills its new output; any other keeps its distance
    /// from the corner (see `perspicax_policy::carry`). Per output, it joins
    /// the workspace its new monitor is showing.
    pub(crate) fn move_to_output(&mut self, window: &Window, towards: Towards) {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let areas: Vec<Rect> = outputs
            .iter()
            .map(|output| rect(self.space.output_geometry(output).unwrap_or_default()))
            .collect();
        let Some(from) = self
            .output_of(window)
            .and_then(|current| outputs.iter().position(|output| *output == current))
        else {
            return;
        };
        let (Some(to), Some(bounds)) = (neighbour(&areas, from, towards), self.extent(window))
        else {
            return;
        };
        let filled = window.toplevel().filter(|t| Self::is_filling(t)).cloned();
        if let Some(toplevel) = filled {
            let state = if toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Fullscreen)
            }) {
                xdg_toplevel::State::Fullscreen
            } else {
                xdg_toplevel::State::Maximized
            };
            self.space
                .map_element(window.clone(), (areas[to].x, areas[to].y), false);
            self.fill(&toplevel, state, None);
        } else {
            let at = carry(rect(bounds), areas[from], areas[to]);
            self.space.map_element(window.clone(), at, false);
        }
        self.window_moved(window);
        self.backend.redraw();
        self.publish_facts();
    }

    /// Take a window off the screen, keeping its place. Focus moves to
    /// whatever is on top now, as it would if the window had closed.
    pub(crate) fn minimize(&mut self, window: &Window) {
        placement(window, |placement| placement.minimized = true);
        self.show_what_belongs();
        self.backend.redraw();
        self.publish_facts();
    }

    /// Bring a parked window back: un-minimize it, and if it is on a
    /// workspace that is not showing, show that workspace, the way activating
    /// a window on another desktop does on every desktop that has them. It
    /// comes back where it was, or onto the nearest monitor if that one is
    /// gone. The caller raises and focuses it, if that is what it wants.
    pub(crate) fn restore(&mut self, window: &Window) {
        placement(window, |placement| placement.minimized = false);
        self.go_to_workspace_of(window);
        self.show_what_belongs();
        self.backend.redraw();
    }

    /// A parked window with this id: minimized, or on another workspace.
    pub(crate) fn parked_with(&self, id: SurfaceId) -> Option<Window> {
        self.parked
            .iter()
            .find(|window| id_of(window) == Some(id))
            .cloned()
    }

    /// Whether the person minimized this window.
    pub(crate) fn is_minimized(window: &Window) -> bool {
        placement(window, |placement| placement.minimized)
    }

    /// The last commit of a resize from the left or top: move the window so
    /// its *opposite* edge stays still, at whatever size the client chose.
    pub(crate) fn settle_resize(&mut self, window: &Window) {
        let Some(resize) = placement(window, |placement| placement.resize) else {
            return;
        };
        let size = window.geometry().size;
        let (x, y) = perspicax_policy::anchor(resize.start, resize.edges, (size.w, size.h));
        if (resize.edges.left || resize.edges.top)
            && self.space.element_location(window) != Some((x, y).into())
        {
            self.space.map_element(window.clone(), (x, y), false);
        }
        if resize.released {
            placement(window, |placement| placement.resize = None);
            self.publish_facts();
        }
    }

    /// Start moving a window with the pointer, from a client's titlebar drag
    /// or a modifier-drag.
    pub(crate) fn start_move(
        &mut self,
        window: &Window,
        start: GrabStartData<Self>,
        serial: Serial,
    ) {
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        // A maximized window dragged by its titlebar comes out of maximized
        // under the pointer, rather than being moved while still claiming to
        // fill the screen.
        if let Some(toplevel) = window.toplevel().filter(|t| Self::is_filling(t)).cloned() {
            let filled = self.space.element_geometry(window);
            let restored = placement(window, |placement| placement.restore.map(|r| r.size));
            if let (Some(filled), Some(restored)) = (filled, restored) {
                let at = unmaximized_at(
                    (start.location.x, start.location.y),
                    rect(filled),
                    (restored.w, restored.h),
                );
                toplevel.with_pending_state(|pending| {
                    pending.states.unset(xdg_toplevel::State::Fullscreen);
                });
                self.unfill(&toplevel, xdg_toplevel::State::Maximized, Some(at.into()));
            }
        }
        let Some(origin) = self.space.element_location(window) else {
            return;
        };
        let grab = MoveGrab::new(start, window.clone(), origin);
        pointer.set_grab(self, grab, serial, Focus::Clear);
        // After, not before: replacing a grab unsets the one before it.
        self.dragging = Some(window.clone());
    }

    /// Start resizing a window with the pointer.
    pub(crate) fn start_resize(
        &mut self,
        window: &Window,
        edges: Edges,
        start: GrabStartData<Self>,
        serial: Serial,
    ) {
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        if window.toplevel().is_some_and(Self::is_filling) {
            return;
        }
        let Some(bounds) = self.space.element_geometry(window) else {
            return;
        };
        let grab = ResizeGrab::new(start, window.clone(), edges, rect(bounds));
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Keep a popup on screen: flip or slide it, as its positioner allows, so
    /// it fits inside the output its window is on.
    pub(crate) fn constrain(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Some(window) = find_popup_root_surface(&kind)
            .ok()
            .and_then(|root| self.window_for(&root))
        else {
            return;
        };
        let (Some(bounds), Some(area)) = (
            self.space.element_geometry(&window),
            self.output_of(&window)
                .and_then(|output| self.space.output_geometry(&output)),
        ) else {
            return;
        };
        // The positioner works in the coordinates of the popup's parent, so
        // the output is expressed there too.
        let mut target = area;
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= bounds.loc;
        popup.with_pending_state(|pending| {
            pending.geometry = pending.positioner.get_unconstrained_geometry(target);
        });
    }

    /// Route the keyboard and pointer to a popup chain (a menu, say) until it
    /// is dismissed, so a click outside closes it the way every desktop does.
    pub(crate) fn grab_popup(&mut self, popup: PopupSurface, seat: &Seat<Self>, serial: Serial) {
        let kind = PopupKind::Xdg(popup);
        let Some(root) = find_popup_root_surface(&kind)
            .ok()
            .filter(|root| self.window_for(root).is_some())
        else {
            return;
        };
        let Ok(mut grab) = self
            .popups
            .grab_popup::<Self>(root.into(), kind, seat, serial)
        else {
            return;
        };
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn pointer_location(&self) -> Option<Point<f64, Logical>> {
        self.pointer
            .as_ref()
            .map(|pointer| pointer.current_location())
    }
}

/// A window's size, as [`Compositor::extent`] measures it, whether it is on
/// screen or parked.
pub(crate) fn extent_size(window: &Window) -> Size<i32, Logical> {
    let declared = window
        .toplevel()
        .and_then(|toplevel| crate::state::declared_geometry(toplevel.wl_surface()))
        .filter(|declared| !declared.is_empty());
    match declared {
        // Whole logical pixels: xdg window geometry is declared in them.
        Some(declared) => (
            (declared.x1 - declared.x0).round() as i32,
            (declared.y1 - declared.y0).round() as i32,
        )
            .into(),
        None => window.geometry().size,
    }
}

pub(crate) fn id_of(window: &Window) -> Option<SurfaceId> {
    window.user_data().get::<SurfaceId>().copied()
}

/// xdg-shell's resize edge, as policy's.
pub(crate) fn edges(edge: xdg_toplevel::ResizeEdge) -> Edges {
    use xdg_toplevel::ResizeEdge as E;
    Edges {
        top: matches!(edge, E::Top | E::TopLeft | E::TopRight),
        bottom: matches!(edge, E::Bottom | E::BottomLeft | E::BottomRight),
        left: matches!(edge, E::Left | E::TopLeft | E::BottomLeft),
        right: matches!(edge, E::Right | E::TopRight | E::BottomRight),
    }
}

/// Whether `surface` is this window's own surface -- its xdg toplevel, or the
/// surface Xwayland associated with its X11 window.
pub(crate) fn is_toplevel_of(window: &Window, surface: &WlSurface) -> bool {
    window.wl_surface().is_some_and(|own| *own == *surface)
}

/// A window's own surface, whichever protocol it came in by. `None` for an
/// X11 window Xwayland has not yet associated with a surface.
pub(crate) fn surface_of(window: &Window) -> Option<WlSurface> {
    window.wl_surface().map(std::borrow::Cow::into_owned)
}
