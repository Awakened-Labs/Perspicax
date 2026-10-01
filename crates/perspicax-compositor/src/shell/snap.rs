//! Snapping, carried out: a window given half or a quarter of a monitor by
//! dragging it to an edge of the desk or by Logo and an arrow.
//!
//! Where the zones are is `perspicax_policy::snap`'s decision. Here a snap is
//! a configure at the zone's size, with xdg-shell's tiled states so a
//! client-side-decorated window drops its shadow and rounded corners on the
//! edges it is tiled against. The top zone is maximize, through
//! [`Compositor::fill`], so a window snapped to the top is a maximized window
//! in every way a client or the index can see. A snapped window remembers
//! where it was in the same [`super::Placement::restore`] maximize uses, so
//! dragging it out of its zone gives it back its size, whichever it was.

use perspicax_policy::{Direction, Snapping, Zone, keyed, zone};
use smithay::{
    desktop::Window,
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle},
};

use super::{placement, rect};
use crate::state::Compositor;

/// The xdg-shell states a zone is tiled against.
const TILED: [xdg_toplevel::State; 4] = [
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

fn tiled(zone: Zone) -> Vec<xdg_toplevel::State> {
    use xdg_toplevel::State::{TiledBottom, TiledLeft, TiledRight, TiledTop};
    match zone {
        Zone::Top => Vec::new(),
        Zone::Left => vec![TiledLeft, TiledTop, TiledBottom],
        Zone::Right => vec![TiledRight, TiledTop, TiledBottom],
        Zone::TopLeft => vec![TiledLeft, TiledTop],
        Zone::TopRight => vec![TiledRight, TiledTop],
        Zone::BottomLeft => vec![TiledLeft, TiledBottom],
        Zone::BottomRight => vec![TiledRight, TiledBottom],
    }
}

/// Where a dragged window would snap if it were let go now: what the seat
/// draws as a preview, and what the release applies.
#[derive(Debug, Clone)]
pub(crate) struct SnapPreview {
    pub(crate) zone: Zone,
    pub(crate) output: Output,
    /// The zone's rectangle in the global space.
    pub(crate) area: Rectangle<i32, Logical>,
}

impl Compositor {
    /// Snap a window to a zone of `output`, or of the output it is mostly
    /// on.
    pub(crate) fn snap(&mut self, window: &Window, zone: Zone, output: Option<Output>) {
        if zone == Zone::Top {
            self.clear_tiling(window);
            // Maximized on the output it is mostly on, which for a window
            // dragged to the top of a monitor is that monitor.
            if let Some(toplevel) = window.toplevel().cloned() {
                self.fill(&toplevel, xdg_toplevel::State::Maximized, None);
                return;
            }
        }
        let Some(output) = output.or_else(|| self.output_of(window)) else {
            return;
        };
        let Some(area) = self.usable_area(&output) else {
            return;
        };
        let target = zone.rect(rect(area));
        if let Some(current) = self.extent(window) {
            placement(window, |placement| {
                placement.restore.get_or_insert(current);
            });
        }
        placement(window, |placement| placement.snapped = Some(zone));
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|pending| {
                pending.states.unset(xdg_toplevel::State::Maximized);
                pending.states.unset(xdg_toplevel::State::Fullscreen);
                for state in TILED {
                    pending.states.unset(state);
                }
                for state in tiled(zone) {
                    pending.states.set(state);
                }
                pending.size = Some((target.w, target.h).into());
            });
            toplevel.send_pending_configure();
        }
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            let _ = x11.configure(Rectangle::new(
                (target.x, target.y).into(),
                (target.w, target.h).into(),
            ));
        }
        self.space
            .map_element(window.clone(), (target.x, target.y), false);
        self.window_moved(window);
        self.backend.redraw();
        self.publish_facts();
    }

    /// Take a window out of its zone: back to the size it had before it was
    /// snapped, at `at`, or where it was.
    pub(crate) fn unsnap(&mut self, window: &Window, at: Option<Point<i32, Logical>>) {
        let restore = placement(window, |placement| {
            placement.snapped = None;
            placement.restore.take()
        });
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|pending| {
                for state in TILED {
                    pending.states.unset(state);
                }
                pending.size = restore.map(|restore| restore.size);
            });
            toplevel.send_pending_configure();
        }
        let Some(restore) = restore else {
            return;
        };
        let to = at.unwrap_or(restore.loc);
        #[cfg(feature = "xwayland")]
        if let Some(x11) = window.x11_surface() {
            let _ = x11.configure(Rectangle::new(to, restore.size));
        }
        self.space.map_element(window.clone(), to, false);
        self.backend.redraw();
        self.publish_facts();
    }

    /// Logo and an arrow, on the focused window.
    pub(crate) fn snap_focused(&mut self, direction: Direction) {
        let Some(window) = self.focused_surface().and_then(|id| self.window_for_id(id)) else {
            return;
        };
        let maximized = window.toplevel().is_some_and(|toplevel| {
            toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Maximized)
            })
        });
        let current = if maximized {
            Some(Zone::Top)
        } else {
            placement(&window, |placement| placement.snapped)
        };
        match (keyed(current, direction), current) {
            (Some(zone), _) => self.snap(&window, zone, None),
            (None, Some(Zone::Top)) => {
                if let Some(toplevel) = window.toplevel().cloned() {
                    self.unfill(&toplevel, xdg_toplevel::State::Maximized, None);
                }
            }
            (None, Some(_)) => self.unsnap(&window, None),
            (None, None) => {}
        }
    }

    /// Whether this window is snapped to a half or a quarter.
    pub(crate) fn is_snapped(window: &Window) -> bool {
        placement(window, |placement| placement.snapped.is_some())
    }

    /// The pointer is at `at` while a window is being dragged: where would it
    /// snap? Kept for the seat to draw, and for the release to apply.
    pub(crate) fn track_snap(&mut self, at: Point<f64, Logical>, snapping: Option<Snapping>) {
        let preview = snapping
            .filter(|snapping| snapping.drag)
            .and_then(|snapping| {
                let output = self.space.output_under(at).next()?.clone();
                let geometry = rect(self.space.output_geometry(&output)?);
                let zone = zone(
                    (at.x, at.y),
                    geometry,
                    &self.output_rects(),
                    snapping.threshold,
                )?;
                let usable = rect(self.usable_area(&output)?);
                let area = zone.rect(usable);
                Some(SnapPreview {
                    zone,
                    output,
                    area: Rectangle::new((area.x, area.y).into(), (area.w, area.h).into()),
                })
            });
        let changed = preview.as_ref().map(|p| (p.zone, p.area))
            != self.snap_preview.as_ref().map(|p| (p.zone, p.area));
        self.snap_preview = preview;
        if changed {
            self.backend.redraw();
        }
    }

    /// The drag ended: snap where the preview said, if anywhere.
    pub(crate) fn finish_snap(&mut self, window: &Window) {
        if let Some(preview) = self.snap_preview.take() {
            self.snap(window, preview.zone, Some(preview.output));
            self.backend.redraw();
        }
    }

    fn clear_tiling(&mut self, window: &Window) {
        placement(window, |placement| placement.snapped = None);
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|pending| {
                for state in TILED {
                    pending.states.unset(state);
                }
            });
        }
    }
}
