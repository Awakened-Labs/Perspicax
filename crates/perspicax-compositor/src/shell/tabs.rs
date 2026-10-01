//! Tab groups, carried out: windows sharing one place on screen, one in
//! front at a time.
//!
//! Which window is in which group, and which is in front, is
//! `perspicax_policy::Groups`'s. Here a tab coming to the front takes over the
//! place of the one it replaces: where it was, how big, whether maximized or
//! snapped, and which workspace it was on. The one it replaces is parked,
//! like a window on a hidden workspace, by the same single rule in
//! `shell::workspaces` that decides what is on screen.

use perspicax_node::SurfaceId;
use smithay::{reexports::wayland_protocols::xdg::shell::server::xdg_toplevel, utils::Point};

use super::{extent_size, id_of, placement, surface_of};
use crate::{framed::Framed, state::Compositor};

impl Compositor {
    /// The window with this id, on screen or parked.
    pub(crate) fn any_window(&self, id: SurfaceId) -> Option<Framed> {
        self.window_for_id(id).or_else(|| self.parked_with(id))
    }

    /// Why a parked window is off screen, as the facts say it: the workspace
    /// it is on, if that is not showing, and the tab in front of it, if it is
    /// a tab behind one. A tab behind takes its workspace from the tab in
    /// front, whose workspace is the group's.
    pub(crate) fn hidden_why(&self, window: &Framed) -> (Option<u16>, Option<SurfaceId>) {
        let behind = self.behind_tab(window);
        let workspace = match behind.and_then(|front| self.any_window(front)) {
            Some(front) => self.off_workspace(&front),
            None => self.off_workspace(window),
        };
        (workspace, behind)
    }

    /// The tab in front of `window`'s group, if `window` is a tab behind it.
    pub(crate) fn behind_tab(&self, window: &Framed) -> Option<SurfaceId> {
        let id = id_of(window)?;
        let front = self.tabs.front(id);
        (front != id).then_some(front)
    }

    /// Whether `window` is in a tab group.
    pub(crate) fn is_tabbed(&self, window: &Framed) -> bool {
        id_of(window).is_some_and(|id| self.tabs.tabs(id).is_some())
    }

    /// What `window`'s titlebar says: one label per tab of its group, and
    /// which is in front, or its own title alone.
    #[cfg(feature = "seat")]
    pub(crate) fn tab_labels(&self, window: &Framed) -> (Vec<String>, usize) {
        let title = |window: &Framed| Self::window_title(window).unwrap_or_default();
        let group = id_of(window).and_then(|id| Some((id, self.tabs.tabs(id)?)));
        let Some((id, members)) = group else {
            return (vec![title(window)], 0);
        };
        let front = self.tabs.front(id);
        let labels = members
            .iter()
            .map(|member| {
                self.any_window(*member)
                    .map(|tab| title(&tab))
                    .unwrap_or_default()
            })
            .collect();
        let at = members.iter().position(|member| *member == front);
        (labels, at.unwrap_or(0))
    }

    /// The `n`th tab of `window`'s group, counting from the left.
    #[cfg(feature = "seat")]
    pub(crate) fn tab_at(&self, window: &Framed, n: usize) -> Option<Framed> {
        let id = id_of(window)?;
        self.any_window(*self.tabs.tabs(id)?.get(n)?)
    }

    /// Bring `tab` to the front of its group, in the group's place.
    pub(crate) fn activate_tab(&mut self, tab: &Framed) {
        let Some(id) = id_of(tab) else {
            return;
        };
        let front = self.tabs.front(id);
        if front == id {
            return;
        }
        let Some(place) = self.any_window(front) else {
            return;
        };
        self.tabs.activate(id);
        self.bring_forward(tab, &place);
    }

    /// Start dragging `tab` by its titlebar with the middle button.
    #[cfg(feature = "seat")]
    pub(crate) fn start_tab_drag(
        &mut self,
        tab: &Framed,
        start: smithay::input::pointer::GrabStartData<Self>,
        serial: smithay::utils::Serial,
    ) {
        let Some(pointer) = self.pointer.clone() else {
            return;
        };
        let grab = super::grabs::TabDragGrab::new(start, tab.clone());
        pointer.set_grab(self, grab, serial, smithay::input::pointer::Focus::Clear);
    }

    /// The window whose titlebar a tab dragged to `at` would join, and the
    /// whole of that window's frame, to outline while it is the target. Not
    /// the dragged window itself, which is under the pointer as it moves, and
    /// not another tab of its own group.
    #[cfg(feature = "seat")]
    pub(crate) fn tab_target(
        &self,
        dragged: &Framed,
        at: Point<f64, smithay::utils::Logical>,
    ) -> Option<(
        SurfaceId,
        smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    )> {
        use perspicax_policy::Part;
        let dragged_id = id_of(dragged)?;
        let window = self.space.elements().rev().find(|window| {
            *window != dragged
                && self
                    .space
                    .element_bbox(window)
                    .is_some_and(|bbox| bbox.to_f64().contains(at))
                && matches!(
                    self.frame_part(window, at),
                    Some(Part::Title | Part::Tab(_))
                )
        })?;
        let target = id_of(window)?;
        if self.tabs.front(dragged_id) == self.tabs.front(target) {
            return None;
        }
        let client = self.extent(window)?;
        let outer = perspicax_policy::outset(super::rect(client), self.insets(window));
        Some((
            target,
            smithay::utils::Rectangle::new((outer.x, outer.y).into(), (outer.w, outer.h).into()),
        ))
    }

    /// A tab dragged with the middle button was let go at `at`, having been
    /// picked up from `home`. Onto another window's titlebar, it joins that
    /// window's group. Anywhere else, it leaves its own group, where it was
    /// dropped, and the tab after it takes the group's place at `home`.
    #[cfg(feature = "seat")]
    pub(crate) fn drop_tab(
        &mut self,
        dragged: &Framed,
        at: Point<f64, smithay::utils::Logical>,
        home: Point<i32, smithay::utils::Logical>,
    ) {
        let Some(id) = id_of(dragged) else {
            return;
        };
        if let Some((target, _)) = self.tab_target(dragged, at) {
            self.attach_tab(id, target);
            return;
        }
        let dropped = self.space.element_location(dragged);
        if self.tabs.tabs(id).is_some() {
            // The tab after it takes the group's place, which is where the
            // dragged one was picked up from, not where it is now.
            self.space.map_element(dragged.clone(), home, false);
            if let Some(heir) = self.tabs.detach(id).and_then(|heir| self.any_window(heir)) {
                self.bring_forward(&heir, dragged);
            }
            self.show_what_belongs();
            if let Some(dropped) = dropped {
                self.space.map_element(dragged.clone(), dropped, false);
            }
        }
        self.fit_frame(dragged);
        self.window_moved(dragged);
        self.space.raise_element(dragged, false);
        self.focus_window(dragged);
        self.backend.redraw();
        self.publish_facts();
    }

    /// Make the focused window a tab of the window focused before it.
    pub(crate) fn tab_with_previous(&mut self) {
        let Some(focused) = self.focused_surface() else {
            return;
        };
        let previous = self
            .space
            .elements()
            .filter_map(id_of)
            .filter(|id| *id != focused)
            .filter_map(|id| Some((self.focused_at(id)?, id)))
            .max()
            .map(|(_, id)| id);
        if let Some(previous) = previous {
            self.attach_tab(focused, previous);
        }
    }

    /// Make `window` a tab of `to`'s group, in front, in the place the
    /// group's front tab had.
    pub(crate) fn attach_tab(&mut self, window: SurfaceId, to: SurfaceId) {
        let Some(place) = self.any_window(self.tabs.front(to)) else {
            return;
        };
        let Some(joining) = self.any_window(window) else {
            return;
        };
        self.tabs.attach(window, to);
        self.bring_forward(&joining, &place);
    }

    /// Bring the next tab of the focused window's group to the front, or the
    /// previous one.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        let Some(focused) = self.focused_surface() else {
            return;
        };
        let Some(next) = self.tabs.cycle(focused, forward) else {
            return;
        };
        if let (Some(next), Some(place)) = (self.any_window(next), self.window_for_id(focused)) {
            self.bring_forward(&next, &place);
        }
    }

    /// Take the focused window out of its tab group. The tab after it takes
    /// the group's place, and it moves down and right of it, so both can be
    /// seen.
    pub(crate) fn detach_tab(&mut self) {
        const ASIDE: i32 = 32;
        let Some(focused) = self.focused_surface() else {
            return;
        };
        let Some(window) = self.window_for_id(focused) else {
            return;
        };
        let Some(heir) = self.tabs.detach(focused) else {
            return;
        };
        if let Some(heir) = self.any_window(heir) {
            self.bring_forward(&heir, &window);
        }
        if let Some(at) = self.space.element_location(&window) {
            self.space
                .map_element(window.clone(), at + Point::from((ASIDE, ASIDE)), false);
            self.fit_frame(&window);
        }
        self.focus_window(&window);
        self.backend.redraw();
        self.publish_facts();
    }

    /// A window is closing: if it was the front of a tab group, the tab after
    /// it comes forward into its place.
    pub(crate) fn tab_closing(&mut self, window: &Framed) {
        let Some(heir) = id_of(window).and_then(|id| self.tabs.detach(id)) else {
            return;
        };
        if let Some(heir) = self.any_window(heir) {
            self.bring_forward(&heir, window);
        }
    }

    /// Put `front` where `place` is, as the group's front tab: on its
    /// workspace, at its location and size, maximized or snapped as it is.
    /// `place` is parked if the groups say it is no longer in front.
    ///
    /// A group has one state, and the tab coming forward takes all of it,
    /// dropping its own: the rect it would be restored to, and whether it is
    /// maximized or tiled. A tab that was maximized while it was in front
    /// otherwise came back maximized into a group that had since been
    /// restored, and restoring it went back to its own stale rect.
    fn bring_forward(&mut self, front: &Framed, place: &Framed) {
        let (Some(id), Some(from)) = (id_of(front), id_of(place)) else {
            return;
        };
        let location = self
            .space
            .element_location(place)
            .or_else(|| placement(place, |placement| placement.parked));
        let size = extent_size(place);
        let (zone, restore) = placement(place, |placement| (placement.snapped, placement.restore));
        let maximized = place.toplevel().is_some_and(|toplevel| {
            toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Maximized)
            })
        });

        self.workspaces.share(id, from);
        self.show_what_belongs();
        // Before the zone or the fill below, which keep a restore rect that is
        // already there rather than taking the tab's own size.
        placement(front, |placement| {
            placement.restore = restore;
            placement.snapped = None;
        });

        let toplevel = front.toplevel().cloned();
        match (zone, toplevel) {
            (Some(zone), _) => self.snap(front, zone, None),
            (None, Some(toplevel)) if maximized => {
                self.fill(&toplevel, xdg_toplevel::State::Maximized, None);
            }
            (None, toplevel) => {
                if let Some(location) = location {
                    if let Some(toplevel) = toplevel {
                        toplevel.with_pending_state(|pending| {
                            pending.states.unset(xdg_toplevel::State::Maximized);
                            pending.states.unset(xdg_toplevel::State::Fullscreen);
                            for state in super::snap::TILED {
                                pending.states.unset(state);
                            }
                            pending.size = Some(size);
                        });
                        toplevel.send_pending_configure();
                    }
                    #[cfg(feature = "xwayland")]
                    if let Some(x11) = front.x11_surface() {
                        let _ = x11.configure(smithay::utils::Rectangle::new(location, size));
                    }
                    self.space.map_element(front.clone(), location, false);
                }
            }
        }
        self.space.raise_element(front, false);
        self.focus_window(front);
        self.backend.redraw();
        self.publish_facts();
    }

    /// Give `window` the keyboard.
    fn focus_window(&mut self, window: &Framed) {
        if let (Some(surface), Some(id)) = (surface_of(window), id_of(window)) {
            self.focus_surface(surface, id);
        }
    }
}
