//! Where the monitors are, and keeping windows on them.
//!
//! Both backends end up here whenever their set of outputs changes: a seat
//! when a monitor is plugged in or out, or the config is reloaded; headless
//! when it starts, or a virtual monitor is plugged in or out. Where each one
//! goes is decided by `perspicax_policy::arrange` from every output's size and
//! its config rule, all at once, because a monitor placed beside another
//! can only be placed once that one has been.
//!
//! Moving a monitor moves its windows with it. Re-arranging is not something a
//! person does to their windows: if the monitor on the left is unplugged and
//! the one on the right slides to the origin, the windows on it slide too. The
//! windows left on a monitor that is gone are then rescued onto the nearest
//! one that remains.

use perspicax_policy::{Rect, Screen, arrange, overlapping, rescue};
use smithay::{
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{framed::Framed, shell::rect, state::Compositor};

impl Compositor {
    /// Place every output where its rule says, carry each window along with
    /// the output it is on, and rescue every window that is on none.
    pub(crate) fn arrange_outputs(&mut self) {
        let placements = self.backend.placements();

        // Which output each window is on, before anything moves, and where
        // that output was. A window on an output that has just been unplugged
        // is on none, and is rescued below rather than carried.
        let before: Vec<(Framed, Output, Point<i32, Logical>)> = self
            .space
            .elements()
            .filter_map(|window| {
                let output = self.mostly_on(window)?;
                let was = self.space.output_geometry(&output)?.loc;
                Some((window.clone(), output, was))
            })
            .collect();

        let names: Vec<String> = placements.iter().map(|(output, _)| output.name()).collect();
        let screens: Vec<Screen<'_>> = placements
            .iter()
            .zip(&names)
            .map(|((output, place), name)| Screen {
                name,
                size: logical_size(output).into(),
                place,
            })
            .collect();
        let positions = arrange(&screens);

        for ((output, _), (x, y)) in placements.iter().zip(positions) {
            let at = Point::from((x, y));
            let mapped = self.space.output_geometry(output).map(|area| area.loc);
            if mapped != Some(at) {
                output.change_current_state(None, None, None, Some(at));
                self.space.map_output(output, at);
                tracing::info!(output = output.name(), ?at, "output placed");
            }
        }
        let areas = self.output_rects();
        if let Some((a, b)) = overlapping(&areas) {
            tracing::warn!(
                first = names.get(a),
                second = names.get(b),
                "two outputs overlap; a window on the overlap is on both"
            );
        }

        for (window, output, was) in before {
            let Some(now) = self.space.output_geometry(&output).map(|area| area.loc) else {
                continue;
            };
            if now != was
                && let Some(location) = self.space.element_location(&window)
            {
                self.space
                    .map_element(window, location + (now - was), false);
            }
        }
        self.rescue_windows();
        // Per output, a rescued window joined its new monitor's workspace,
        // and a monitor that is gone took its own workspace with it.
        self.show_what_belongs();

        self.backend.redraw();
        self.publish_facts();
    }

    /// Every mapped output's rect in the global space.
    pub(crate) fn output_rects(&self) -> Vec<Rect> {
        self.space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .map(rect)
            .collect()
    }

    /// Bring every window that is on no output onto the nearest one. A
    /// maximized or fullscreen window fills the output it lands on, rather
    /// than keeping the size of the monitor it filled.
    fn rescue_windows(&mut self) {
        let areas = self.output_rects();
        let strays: Vec<(Framed, Point<i32, Logical>)> = self
            .space
            .elements()
            .filter_map(|window| {
                let bounds = self.extent(window)?;
                let (x, y) = rescue(rect(bounds), &areas)?;
                let location = self.space.element_location(window)?;
                Some((
                    window.clone(),
                    location + (Point::from((x, y)) - bounds.loc),
                ))
            })
            .collect();
        for (window, at) in strays {
            tracing::info!(?at, "a window on no output is brought back onto one");
            self.space.map_element(window.clone(), at, false);
            self.window_moved(&window);
            let Some(toplevel) = window.toplevel().filter(|t| Self::is_filling(t)).cloned() else {
                self.fit_frame(&window);
                continue;
            };
            let state = if toplevel.with_pending_state(|pending| {
                pending.states.contains(xdg_toplevel::State::Fullscreen)
            }) {
                xdg_toplevel::State::Fullscreen
            } else {
                xdg_toplevel::State::Maximized
            };
            self.fill(&toplevel, state, None);
        }
    }

    /// Where a minimized window should come back: where it was, unless the
    /// monitor it was on is gone, and then onto the nearest one.
    pub(crate) fn unpark(
        &self,
        window: &Framed,
        parked: Point<i32, Logical>,
    ) -> Point<i32, Logical> {
        let size = window.geometry().size;
        let bounds = Rect::new(parked.x, parked.y, size.w.max(1), size.h.max(1));
        rescue(bounds, &self.output_rects()).map_or(parked, Point::from)
    }

    /// The output most of this window is on, or `None` if none of it is on
    /// any. Unlike `output_of`, no fallback to the pointer's: a window on no
    /// output is exactly what has to be told apart here.
    fn mostly_on(&self, window: &Framed) -> Option<Output> {
        self.output_at(self.extent(window)?)
    }

    /// The output most of `bounds` is on, or `None` if none of it is.
    pub(crate) fn output_at(&self, bounds: Rectangle<i32, Logical>) -> Option<Output> {
        self.space
            .outputs()
            .filter_map(|output| {
                let shared = bounds.intersection(self.space.output_geometry(output)?)?;
                Some((output, i64::from(shared.size.w) * i64::from(shared.size.h)))
            })
            .max_by_key(|&(_, area)| area)
            .map(|(output, _)| output.clone())
    }
}

/// An output's size in the global space: its mode, turned by its transform
/// and divided by its scale. A 4K monitor at scale 2 is 1920 wide here.
/// Rounded up, as `Space::output_geometry` rounds it, so the two agree.
fn logical_size(output: &Output) -> Size<i32, Logical> {
    let Some(mode) = output.current_mode() else {
        return Size::default();
    };
    output
        .current_transform()
        .transform_size(mode.size)
        .to_f64()
        .to_logical(output.current_scale().fractional_scale())
        .to_i32_ceil()
}
