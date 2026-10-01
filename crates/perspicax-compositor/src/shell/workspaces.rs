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
//! A parked window keeps where it was in its [`super::Placement`], and that
//! is also how its monitor is known while it is off screen: per output, a
//! window's workspace is a cell of the grid of the monitor it sits on.

use perspicax_policy::{Cell, Direction, Home};
use smithay::utils::Rectangle;

use super::{extent_size, id_of, placement, surface_of};
use crate::{framed::Framed, state::Compositor};

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

    /// A window has closed: off the screen, out of the parked list, and out
    /// of the workspaces' bookkeeping.
    pub(crate) fn forget_window(&mut self, window: &Framed) {
        self.space.unmap_elem(window);
        self.parked.retain(|parked| parked != window);
        if let Some(id) = id_of(window) {
            self.workspaces.remove(id);
        }
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
    /// a window closes.
    pub(crate) fn show_what_belongs(&mut self) {
        let windows: Vec<Framed> = self.space.elements().chain(&self.parked).cloned().collect();
        for window in windows {
            let Some(id) = id_of(&window) else {
                continue;
            };
            let belongs = !Self::is_minimized(&window)
                && self
                    .monitor_of(&window)
                    .is_none_or(|output| self.workspaces.shows(id, &output));
            let shown = self.space.elements().any(|mapped| *mapped == window);
            if shown && !belongs {
                self.park(&window);
            } else if !shown && belongs {
                self.unpark_window(&window);
            }
        }

        let lost = self.keyboard_focus().is_some_and(|focus| {
            self.parked
                .iter()
                .any(|parked| surface_of(parked).as_ref() == Some(&focus))
        });
        if lost {
            self.focus_top_window();
        }
    }

    /// Switch the workspace showing on the monitor the person is working on
    /// (every monitor, when workspaces span them).
    pub(crate) fn switch_workspace(&mut self, direction: Direction) {
        let Some(output) = self.working_monitor() else {
            return;
        };
        if self.workspaces.switch(&output, direction).is_some() {
            self.workspace_changed();
        }
    }

    /// Flip the workspace on `output` one step, as the pointer resting on an
    /// edge of the desk does, taking the window being dragged along if there
    /// is one. Whether it flipped: not off the edge of a grid that does not
    /// wrap.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's pointer")
    )]
    pub(crate) fn flip(
        &mut self,
        output: &str,
        direction: Direction,
        carrying: Option<&Framed>,
    ) -> bool {
        let output = output.to_owned();
        let flipped = match carrying.and_then(id_of) {
            Some(id) => self.workspaces.carry(id, &output, direction),
            None => self.workspaces.switch(&output, direction),
        };
        if flipped.is_some() {
            self.workspace_changed();
        }
        flipped.is_some()
    }

    /// Step the workspace on `output` to the next one in reading order, or
    /// the previous, as a scroll over the desktop does.
    #[cfg_attr(
        not(feature = "seat"),
        expect(dead_code, reason = "the seat's pointer")
    )]
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

    /// Show the workspace a person calls `number`.
    pub(crate) fn go_to_workspace(&mut self, number: u16) {
        let (Some(output), Some(cell)) = (
            self.working_monitor(),
            self.workspaces.shape().grid.numbered(number),
        ) else {
            return;
        };
        if self.workspaces.go_to(&output, cell).is_some() {
            self.workspace_changed();
        }
    }

    /// Move the focused window a workspace across the grid, and go with it
    /// (`follow`) or stay.
    pub(crate) fn send_to_workspace(&mut self, direction: Direction, follow: bool) {
        let Some(window) = self.focused_surface().and_then(|id| self.window_for_id(id)) else {
            return;
        };
        let (Some(id), Some(output)) = (id_of(&window), self.monitor_of(&window)) else {
            return;
        };
        let moved = if follow {
            self.workspaces.carry(id, &output, direction)
        } else {
            self.workspaces.send(id, &output, direction)
        };
        if moved.is_some() {
            self.workspace_changed();
        }
    }

    /// Put the focused window on every workspace, or back on one.
    pub(crate) fn toggle_sticky(&mut self) {
        let Some(window) = self.focused_surface().and_then(|id| self.window_for_id(id)) else {
            return;
        };
        if let (Some(id), Some(output)) = (id_of(&window), self.monitor_of(&window)) {
            self.workspaces.toggle_sticky(id, &output);
            self.workspace_changed();
        }
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

    fn workspace_changed(&mut self) {
        self.show_what_belongs();
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
        self.parked.push(window.clone());
    }

    /// Map a parked window back where it was, or onto the nearest monitor if
    /// that one is gone.
    fn unpark_window(&mut self, window: &Framed) {
        self.parked.retain(|parked| parked != window);
        let parked = placement(window, |placement| placement.parked.take());
        let at = self.unpark(window, parked.unwrap_or_default());
        self.space.map_element(window.clone(), at, false);
        self.fit_frame(window);
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

    /// The monitor the person is working on: the focused window's, or the
    /// one under the pointer.
    fn working_monitor(&self) -> Option<String> {
        self.focused_surface()
            .and_then(|id| self.window_for_id(id))
            .and_then(|window| self.monitor_of(&window))
            .or_else(|| self.output_under_pointer().map(|output| output.name()))
    }
}
