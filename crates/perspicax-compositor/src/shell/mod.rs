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

mod grabs;

use std::cell::RefCell;

use perspicax_node::SurfaceId;
use perspicax_policy::{Edges, Rect, place, unmaximized_at};
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
    utils::{Logical, Point, Rectangle, Serial},
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
    /// Where it was when it was minimized. A minimized window is unmapped
    /// from the space, and this is how it gets its place back.
    pub(crate) parked: Option<Point<i32, Logical>>,
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
        let output = self
            .pointer_location()
            .and_then(|at| self.space.output_under(at).next().cloned())
            .or_else(|| self.space.outputs().next().cloned());
        let area = output
            .and_then(|output| self.space.output_geometry(&output))
            .unwrap_or_default();
        let at = place(self.placed, rect(area));
        self.placed = self.placed.wrapping_add(1);
        at.into()
    }

    /// The output a window is mostly on, or the one under the pointer.
    pub(crate) fn output_of(&self, window: &Window) -> Option<Output> {
        let bounds = self.space.element_geometry(window);
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
    /// draws (a fullscreen window drops its own decorations) and in stacking,
    /// since fullscreen is always raised. The rectangle they fill is the same
    /// until layer-shell (slice 6) gives maximized windows an area that
    /// excludes panels.
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
        let Some(area) = output.and_then(|output| self.space.output_geometry(&output)) else {
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

    /// Take a window off the screen, keeping its place. Focus moves to
    /// whatever is on top now, as it would if the window had closed.
    pub(crate) fn minimize(&mut self, window: &Window) {
        let Some(at) = self.space.element_location(window) else {
            return;
        };
        placement(window, |placement| placement.parked = Some(at));
        self.space.unmap_elem(window);
        self.minimized.push(window.clone());
        let top = self.space.elements().last().cloned();
        match top
            .as_ref()
            .and_then(|top| Some((top.wl_surface()?.into_owned(), id_of(top)?)))
        {
            Some((surface, id)) => self.focus_surface(surface, id),
            None => {
                if let Some(keyboard) = self.keyboard.clone() {
                    keyboard.set_focus(self, None, smithay::utils::SERIAL_COUNTER.next_serial());
                }
            }
        }
        self.backend.redraw();
        self.publish_facts();
    }

    /// Put a minimized window back where it was. The caller raises and
    /// focuses it, if that is what it wants.
    pub(crate) fn restore(&mut self, window: &Window) {
        let Some(at) = self.minimized.iter().position(|w| w == window) else {
            return;
        };
        let window = self.minimized.remove(at);
        let parked = placement(&window, |placement| placement.parked.take());
        self.space
            .map_element(window, parked.unwrap_or_default(), false);
        self.backend.redraw();
    }

    /// A minimized window with this id.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's input path")
    )]
    pub(crate) fn minimized_with(&self, id: SurfaceId) -> Option<Window> {
        self.minimized
            .iter()
            .find(|window| id_of(window) == Some(id))
            .cloned()
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
        let Ok(mut grab) = self.popups.grab_popup::<Self>(root, kind, seat, serial) else {
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

/// Whether `surface` is this window's toplevel.
pub(crate) fn is_toplevel_of(window: &Window, surface: &WlSurface) -> bool {
    window
        .toplevel()
        .is_some_and(|toplevel| toplevel.wl_surface() == surface)
}
