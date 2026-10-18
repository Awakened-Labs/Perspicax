//! Workspaces, carried out: which windows are in the space and which are
//! parked.
//!
//! One rule decides it, [`Compositor::show_what_belongs`]: a window is on
//! screen when the person has not minimized it and its workspace is the one
//! its monitor is showing (`perspicax_policy::Workspaces::shows`). Every
//! change (a switch, a window sent away, a minimize, a restore, a monitor
//! unplugged) updates the bookkeeping and then runs the rule, so there is no
//! second path that hides or shows a window and could disagree with it.
//!
//! The window in the person's hand is on whatever workspace its monitor is
//! showing, however that changed: it goes where they go. A window's own menu
//! goes with the window: one parked closes its menu, and the keyboard moves
//! on from it as from the window (#114).
//!
//! A parked window keeps where it was in its [`super::Placement`], and that
//! is also how its monitor is known while it is off screen: per output, a
//! window's workspace is a cell of the grid of the monitor it sits on.
//!
//! The rule changes what is on screen at once. A seat then draws the change
//! over a moment, as the person's `[workspaces.transition]` says: each
//! switch is handed to the backend as the rule finishes, and the workspaces
//! it draws on the way are the parked windows, where they will be mapped
//! back ([`Compositor::pictured`]).

#[cfg(feature = "seat")]
use perspicax_policy::Mode;
use perspicax_policy::{Cell, Direction, Home};
use smithay::utils::Rectangle;
#[cfg(feature = "seat")]
use smithay::{
    desktop::space::SpaceElement as _,
    output::Output,
    utils::{Logical, Point},
};

#[cfg(feature = "seat")]
use super::covers_panels;
use super::{extent_size, id_of, placement};
use crate::{framed::Framed, state::Compositor};

/// The windows a workspace that is not on screen shows on a monitor, front
/// to back, each where its surface's origin is drawn in the monitor's own
/// logical coordinates, as [`super::Stack`] has the ones on screen.
#[cfg(feature = "seat")]
#[derive(Default)]
pub(crate) struct Pictured {
    /// A fullscreen window that was in use when it was left, still over the
    /// panels.
    pub(crate) raised: Vec<(Framed, Point<i32, Logical>)>,
    pub(crate) windows: Vec<(Framed, Point<i32, Logical>)>,
}

impl Compositor {
    /// A window the person will manage has appeared: it joins the workspace
    /// its monitor is showing. Menus and tooltips are never adopted, and so
    /// are shown whatever is showing, with the window they belong to.
    pub(crate) fn adopt(&mut self, window: &Framed) {
        let (Some(id), Some(output)) = (id_of(window), self.monitor_of(window)) else {
            return;
        };
        self.workspaces.add(id, &output);
    }

    /// A window has closed: where it was is its program's place now, and it
    /// is off the screen, out of the parked list, and out of the workspaces'
    /// bookkeeping. Remembered first, while it is still where it was and in
    /// its tab group.
    pub(crate) fn forget_window(&mut self, window: &Framed) {
        self.remember_place(window);
        self.tab_closing(window);
        self.space.unmap_elem(window);
        self.parked.retain(|parked| parked != window);
        if let Some(id) = id_of(window) {
            self.workspaces.remove(id);
            self.layout_memory.forget(id);
        }
    }

    /// Whether `window` is still one of the person's: on screen, or parked.
    /// Not one that closed, nor one its client unmapped.
    pub(crate) fn has_window(&self, window: &Framed) -> bool {
        self.space
            .elements()
            .chain(&self.parked)
            .any(|kept| kept == window)
    }

    /// A window was put somewhere new: dragged, sent to another monitor, or
    /// rescued from one that was unplugged. Per output, it joins the
    /// workspace its new monitor shows, so it stays in front of the person
    /// who moved it.
    pub(crate) fn window_moved(&mut self, window: &Framed) {
        if let (Some(id), Some(output)) = (id_of(window), self.monitor_of(window)) {
            self.workspaces.moved_to(id, &output);
        }
    }

    /// Run the rule: park every window that should not be on screen, bring
    /// back every parked one that should. Then, if the window with the
    /// keyboard was parked, focus moves to whatever is on top, as it does when
    /// a window closes, and so it does from a menu of that window's, which
    /// closes.
    ///
    /// The window being dragged is put on whatever its monitor shows now
    /// before it is judged: a switch with a window in hand takes it along, by
    /// a key, a pager or an edge alike, rather than parking it under the
    /// person's hand.
    pub(crate) fn show_what_belongs(&mut self) {
        let windows: Vec<Framed> = self.space.elements().chain(&self.parked).cloned().collect();
        for window in windows {
            let Some(id) = id_of(&window) else {
                continue;
            };
            let monitor = self.monitor_of(&window);
            let present = !Self::is_minimized(&window) && self.tabs.is_front(id);
            if present
                && self.dragging.as_ref() == Some(&window)
                && let Some(output) = &monitor
            {
                self.workspaces.hold(id, output);
            }
            let belongs =
                present && monitor.is_none_or(|output| self.workspaces.shows(id, &output));
            let shown = self.space.elements().any(|mapped| *mapped == window);
            if shown && !belongs {
                self.park(&window);
            } else if !shown && belongs {
                self.unpark_window(&window);
            }
        }

        // In a menu of a parked window, the keyboard is as lost: the menu
        // goes with its window, closed as a click outside it would close it,
        // which lets go of the keyboard so it can move on. Only a menu that
        // holds it: whatever else holds the pointer, a drag flipping the
        // workspace say, is left be.
        let lost = self
            .window_in_use()
            .is_some_and(|id| self.parked_with(id).is_some());
        if lost {
            if self.held_menu_root().is_some() {
                self.let_go();
                self.repoint();
            }
            self.focus_top_window();
        }
        self.draw_switches();
    }

    /// Hand the switches just made to the backend, to draw over a moment.
    /// Taken every time, drawn or not, so none is drawn long after it was
    /// made. Behind the lock only the lock is drawn: a window brought forward
    /// there switches with nothing to see.
    fn draw_switches(&mut self) {
        let switches = self.workspaces.take_switches();
        if switches.is_empty() || self.lock.is_some() {
            return;
        }
        let grid = self.workspaces.shape().grid;
        let now = u64::try_from(self.started_at().elapsed().as_millis()).unwrap_or(u64::MAX);
        self.backend.begin_transitions(switches, grid, now);
    }

    /// What workspace `cell` shows on `output` while it is off screen and a
    /// transition draws it: its parked windows, not minimized, the front tab
    /// of a group, each where it will be mapped back (onto the nearest
    /// monitor, if its own is gone). Per output, only the ones whose monitor
    /// this is: another monitor's cell of the same number is another
    /// workspace.
    #[cfg(feature = "seat")]
    pub(crate) fn pictured(&self, output: &Output, cell: Cell) -> Pictured {
        let mut pictured = Pictured::default();
        let Some(area) = self.space.output_geometry(output) else {
            return pictured;
        };
        let per_output = self.workspaces.shape().mode == Mode::PerOutput;
        for window in self.parked.iter().rev() {
            let Some(id) = id_of(window) else {
                continue;
            };
            let parked = placement(window, |placement| placement.parked);
            let (Some(Home::On(home)), Some(parked)) = (self.workspaces.home(id), parked) else {
                continue;
            };
            if home != cell || Self::is_minimized(window) || !self.tabs.is_front(id) {
                continue;
            }
            // Where smithay would draw it, mapped there: its origin is the
            // client geometry's offset back from the window's location.
            let origin = self.unpark(window, parked) - window.geometry().loc;
            let mut bbox = window.bbox();
            bbox.loc += origin;
            if !bbox.overlaps(area)
                || (per_output && self.monitor_of(window).as_deref() != Some(&output.name()))
            {
                continue;
            }
            let piece = (window.clone(), origin - area.loc);
            if covers_panels(window) {
                pictured.raised.push(piece);
            } else {
                pictured.windows.push(piece);
            }
        }
        pictured
    }

    /// Whether `window`, on screen, stays where it is while a transition on
    /// `output` moves the rest: one on every workspace, or that was never
    /// given one; the one in the person's hand, or carried by key; and, per
    /// output, one on another monitor, whose workspace is not changing.
    #[cfg(feature = "seat")]
    pub(crate) fn stays_put(
        &self,
        window: &Framed,
        output: &Output,
        carrying: Option<perspicax_node::SurfaceId>,
    ) -> bool {
        let Some(id) = id_of(window) else {
            return true;
        };
        if !matches!(self.workspaces.home(id), Some(Home::On(_)))
            || carrying == Some(id)
            || self.dragging.as_ref() == Some(window)
        {
            return true;
        }
        self.workspaces.shape().mode == Mode::PerOutput
            && self.monitor_of(window).as_deref() != Some(&output.name())
    }

    /// Switch the workspace showing on `monitor` (every monitor, when
    /// workspaces span them).
    pub(crate) fn switch_workspace(&mut self, monitor: Option<String>, direction: Direction) {
        if let Some(output) = monitor {
            self.flip(&output, direction);
        }
    }

    /// Flip the workspace on `output` one step, as a key or the pointer
    /// resting on an edge of the desk does. A window being dragged comes
    /// along (see [`Self::show_what_belongs`]). Whether it flipped: not off
    /// the edge of a grid that does not wrap.
    pub(crate) fn flip(&mut self, output: &str, direction: Direction) -> bool {
        let flipped = self.workspaces.switch(&output.to_owned(), direction);
        if flipped.is_some() {
            self.workspace_changed();
        }
        flipped.is_some()
    }

    /// Step the workspace on `output` to the next one in reading order, or
    /// the previous, as a scroll over the desktop does.
    pub(crate) fn scroll_workspace(&mut self, output: &str, forward: bool) {
        let output = output.to_owned();
        let grid = self.workspaces.shape().grid;
        let Some(to) = grid.next(self.workspaces.current(&output), forward) else {
            return;
        };
        if self.workspaces.go_to(&output, to).is_some() {
            self.workspace_changed();
        }
    }

    /// Show the workspace a person calls `number` on `monitor`.
    pub(crate) fn go_to_workspace(&mut self, monitor: Option<String>, number: u16) {
        let (Some(output), Some(cell)) = (monitor, self.workspaces.shape().grid.numbered(number))
        else {
            return;
        };
        if self.workspaces.go_to(&output, cell).is_some() {
            self.workspace_changed();
        }
    }

    /// Move the focused window a workspace across the grid, and go with it
    /// (`follow`) or stay. A window in the person's hand cannot be left
    /// behind: sent away, it takes them along.
    pub(crate) fn send_to_workspace(&mut self, direction: Direction, follow: bool) {
        let Some(window) = self.focused_surface().and_then(|id| self.window_for_id(id)) else {
            return;
        };
        let (Some(id), Some(output)) = (id_of(&window), self.monitor_of(&window)) else {
            return;
        };
        let follow = follow || self.dragging.as_ref() == Some(&window);
        let moved = if follow {
            self.workspaces.carry(id, &output, direction)
        } else {
            self.workspaces.send(id, &output, direction)
        };
        if moved.is_some() {
            self.workspace_changed();
        }
    }

    /// Put `window` on every workspace, or back on the one its monitor
    /// shows: the focused window, by a key, or the one whose sticky button
    /// was pressed, which need not have the keyboard. A window never given
    /// a workspace is on every one already, and stays so.
    pub(crate) fn toggle_sticky(&mut self, window: &Framed) {
        let (Some(id), Some(output)) = (id_of(window), self.monitor_of(window)) else {
            return;
        };
        if self.workspaces.home(id).is_none() {
            tracing::debug!(
                ?id,
                "not made sticky or not: it has no workspace of its own"
            );
            return;
        }
        self.workspaces.toggle_sticky(id, &output);
        self.workspace_changed();
    }

    /// Show the workspace a window is on, on its monitor, if it is not
    /// showing already.
    pub(crate) fn go_to_workspace_of(&mut self, window: &Framed) {
        let (Some(id), Some(output)) = (id_of(window), self.monitor_of(window)) else {
            return;
        };
        if let Some(Home::On(cell)) = self.workspaces.home(id) {
            self.workspaces.go_to(&output, cell);
        }
    }

    /// Whether a window's workspace is the one its monitor is showing,
    /// minimized or not.
    pub(crate) fn on_showing_workspace(&self, window: &Framed) -> bool {
        let (Some(id), Some(output)) = (id_of(window), self.monitor_of(window)) else {
            return true;
        };
        self.workspaces.shows(id, &output)
    }

    /// The workspace a parked window is on, counting from 1, if that is why
    /// it is parked: what the index is told, so an agent hears "on workspace
    /// 3" rather than "not mapped".
    pub(crate) fn off_workspace(&self, window: &Framed) -> Option<u16> {
        if Self::is_minimized(window) {
            return None;
        }
        let id = id_of(window)?;
        let output = self.monitor_of(window)?;
        match self.workspaces.home(id)? {
            Home::On(Cell(cell)) if !self.workspaces.shows(id, &output) => Some(cell + 1),
            _ => None,
        }
    }

    pub(crate) fn workspace_changed(&mut self) {
        self.show_what_belongs();
        // Leaving for a workspace with no window left nothing with the
        // keyboard. Coming back gives it to the window in front, as it is
        // on any desktop.
        if self.keyboard_focus().is_none() {
            self.focus_top_window();
        }
        self.backend.redraw();
        self.publish_facts();
    }

    /// Unmap a window from the space, keeping its place.
    fn park(&mut self, window: &Framed) {
        let Some(at) = self.space.element_location(window) else {
            return;
        };
        placement(window, |placement| placement.parked = Some(at));
        self.space.unmap_elem(window);
        if !self.parked.contains(window) {
            self.parked.push(window.clone());
        }
    }

    /// Map a parked window back where it was, or onto the nearest monitor if
    /// that one is gone, then fit it to the room it has now: one that fills
    /// its monitor or a zone fills it again, as the panels now leave it, and
    /// one made fullscreen while it was parked becomes it. Mapped, it is not
    /// yet counted on its monitor; the next frame counts it, and tells it it
    /// may draw ([`Compositor::settle_outputs`]).
    fn unpark_window(&mut self, window: &Framed) {
        self.parked.retain(|parked| parked != window);
        let parked = placement(window, |placement| placement.parked.take());
        let at = self.unpark(window, parked.unwrap_or_default());
        self.space.map_element(window.clone(), at, false);
        self.refit(window);
    }

    /// The monitor a window is on, by name, whether it is on screen or
    /// parked: the one most of it overlaps, or the nearest one if it is on
    /// none, which is where it would be rescued to.
    ///
    /// A window that has not drawn yet has no size, and is on the monitor
    /// its top-left corner is on: where it was placed, which is what decides
    /// the workspace it opens on.
    pub(crate) fn monitor_of(&self, window: &Framed) -> Option<String> {
        let mut bounds = self.extent(window).or_else(|| {
            let at = placement(window, |placement| placement.parked)?;
            Some(Rectangle::new(at, extent_size(window)))
        })?;
        bounds.size.w = bounds.size.w.max(1);
        bounds.size.h = bounds.size.h.max(1);
        self.output_at(bounds)
            .or_else(|| {
                let at = self.unpark(window, bounds.loc);
                self.output_at(Rectangle::new(at, bounds.size))
            })
            .map(|output| output.name())
    }

    /// The monitor the person is working on: the window in use's, its own
    /// menu open or not, or the one under the pointer. Where a key binding
    /// acts.
    pub(crate) fn working_monitor(&self) -> Option<String> {
        self.window_in_use()
            .and_then(|id| self.window_for_id(id))
            .and_then(|window| self.monitor_of(&window))
            .or_else(|| self.output_under_pointer().map(|output| output.name()))
    }
}
