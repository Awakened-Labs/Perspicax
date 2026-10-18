//! A workspace switch, drawn over a moment ([`perspicax_policy::Transition`]).
//!
//! The switch is made at once: the windows of the workspace arrived at are
//! in the space where they will stay, and the ones left are parked. Only the
//! picture moves. The arriving workspace's windows are drawn from the space
//! with the transition's offset, alpha and clip, and every other workspace
//! on the way from where its windows are parked
//! ([`Compositor::pictured`]), as they last looked. The panels, the
//! wallpaper and the windows that stay put ([`Compositor::stays_put`]) are
//! drawn as at any other time. A picture an agent or a recorder takes is
//! drawn from the settled stack, and never shows one.
//!
//! A transition is per monitor, on its [`Head`], so per output only the
//! monitor that switched moves, and an unplugged one's goes with it. While
//! one runs its head draws every frame ([`keep_moving`]). Once it ends, a
//! quiet desktop costs nothing again.

use std::time::Duration;

use perspicax_node::SurfaceId;
use perspicax_policy::{Area, Cell, Frame, Grid, Pane, Switched, Transition};
use smithay::{
    backend::renderer::{
        element::{Kind, solid::SolidColorRenderElement, utils::CropRenderElement},
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    output::Output,
    reexports::{
        calloop::{
            LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        drm::control::crtc,
    },
    utils::{Logical, Physical, Point, Rectangle, Size},
};

use super::{Elements, Head, Session, layer_elements, render, window_elements};
use crate::{
    backend::Running,
    framed::Framed,
    shell::{Pictured, Stack},
    state::Compositor,
};

/// How long a frame is when a monitor does not say.
const FALLBACK_FRAME: Duration = Duration::from_millis(16);

/// A wipe's bar.
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A switch being drawn on one monitor.
pub(super) struct Moving {
    transition: Transition,
    /// The window carried there by key, which stays in front of the person.
    carrying: Option<SurfaceId>,
    /// Whether a frame of it has been drawn. Until one has, a switch after
    /// it starts from where this one did: several can come before a frame,
    /// one per notch of a fast scroll, and the person saw none of the
    /// workspaces between.
    drawn: bool,
}

/// What a head draws of its transition this frame, gathered before the
/// compositor is taken apart to draw: the moment, every other workspace it
/// shows, and the windows that stay put.
#[derive(Default)]
pub(super) struct Prepared {
    pub(super) frame: Option<Frame>,
    pictures: Vec<(Cell, Pictured)>,
    still: Vec<Framed>,
}

impl Session {
    /// Start drawing `switches` on the monitors they were made on, every
    /// monitor for a spanning one. A switch while another is drawn takes
    /// over from it, and never waits behind it.
    pub(crate) fn begin_transitions(
        &mut self,
        switches: Vec<Switched<SurfaceId, String>>,
        grid: Grid,
        now: u64,
    ) {
        let how = self.settings.transitioning;
        for switched in switches {
            for head in &mut self.heads {
                if switched
                    .on
                    .as_ref()
                    .is_some_and(|name| *name != head.output.name())
                {
                    continue;
                }
                let undrawn = head.moving.take().filter(|moving| !moving.drawn);
                let (from, heading, carrying) = match undrawn {
                    Some(undrawn) => (
                        undrawn.transition.from(),
                        switched
                            .heading
                            .filter(|&heading| undrawn.transition.heading() == Some(heading)),
                        switched.carrying.or(undrawn.carrying),
                    ),
                    None => (switched.from, switched.heading, switched.carrying),
                };
                head.moving =
                    Transition::new(how, grid, from, switched.to, heading, now).map(|transition| {
                        Moving {
                            transition,
                            carrying,
                            drawn: false,
                        }
                    });
            }
        }
        self.request_frames();
    }

    /// End every transition at once, and draw what is there.
    pub(crate) fn settle_transitions(&mut self) {
        let mut settled = false;
        for head in &mut self.heads {
            settled |= head.moving.take().is_some();
        }
        if settled {
            self.request_frames();
        }
    }
}

/// What the head on `crtc` draws of its transition at `now`: nothing once it
/// is over. The other workspaces' windows are gathered only when the frame
/// will be drawn: not behind the lock, nor while a frame is on its way.
pub(super) fn prepare(state: &Compositor, crtc: crtc::Handle, now: u64) -> Prepared {
    let Running::Seat(session) = &state.backend else {
        return Prepared::default();
    };
    let Some(head) = session.heads.iter().find(|head| head.crtc == crtc) else {
        return Prepared::default();
    };
    let Some(moving) = &head.moving else {
        return Prepared::default();
    };
    let mut prepared = Prepared {
        frame: moving.transition.at(now),
        ..Prepared::default()
    };
    let Some(frame) = &prepared.frame else {
        return prepared;
    };
    if !session.active || head.queued || state.lock.is_some() {
        return prepared;
    }
    let output = &head.output;
    prepared.pictures = frame
        .over
        .iter()
        .chain(&frame.under)
        .map(|pane| (pane.cell, state.pictured(output, pane.cell)))
        .collect();
    prepared.still = state
        .space
        .elements()
        .filter(|window| state.stays_put(window, output, moving.carrying))
        .cloned()
        .collect();
    prepared
}

/// A frame of this head's transition was drawn: a switch made now takes
/// over from it rather than starting from where it did.
pub(super) fn drawn(head: &mut Head) {
    if let Some(moving) = head.moving.as_mut() {
        moving.drawn = true;
    }
}

/// What the head shows at a moment of its transition, front to back: the
/// overlays; the windows over the panels; the panels; a wipe's bar; the
/// workspaces drawn over the arriving one (a crossfade's old one); the
/// arriving one, with the windows that stay put in their places among its
/// own; the workspaces under it; the wallpaper.
pub(super) fn elements(
    renderer: &mut GlesRenderer,
    head: &Head,
    stack: &Stack,
    prepared: &Prepared,
    frame: &Frame,
    scale: f64,
) -> Vec<Elements> {
    let size = head
        .output
        .current_mode()
        .map_or_else(|| Size::from((0, 0)), |mode| mode.size);
    let pictured = |pane: &Pane| {
        prepared
            .pictures
            .iter()
            .find(|(cell, _)| *cell == pane.cell)
            .map(|(_, pictured)| pictured)
    };
    let still = |window: &Framed| prepared.still.contains(window);

    let mut elements = layer_elements(renderer, &stack.overlay, scale);
    for pane in &frame.over {
        if let Some(pictured) = pictured(pane) {
            elements.extend(moved(renderer, &pictured.raised, pane, size, scale));
        }
    }
    elements.extend(arriving(
        renderer,
        &stack.raised,
        frame,
        &still,
        size,
        scale,
    ));
    for pane in &frame.under {
        if let Some(pictured) = pictured(pane) {
            elements.extend(moved(renderer, &pictured.raised, pane, size, scale));
        }
    }
    elements.extend(layer_elements(renderer, &stack.top, scale));
    if let Some(bar) = frame.bar {
        elements.push(Elements::Preview(SolidColorRenderElement::new(
            head.bar.clone(),
            pixels(bar, size),
            CommitCounter::default(),
            BLACK,
            Kind::Unspecified,
        )));
    }
    for pane in &frame.over {
        if let Some(pictured) = pictured(pane) {
            elements.extend(moved(renderer, &pictured.windows, pane, size, scale));
        }
    }
    elements.extend(arriving(
        renderer,
        &stack.windows,
        frame,
        &still,
        size,
        scale,
    ));
    for pane in &frame.under {
        if let Some(pictured) = pictured(pane) {
            elements.extend(moved(renderer, &pictured.windows, pane, size, scale));
        }
    }
    elements.extend(layer_elements(renderer, &stack.lower, scale));
    elements
}

/// The windows on screen, in their own order: those that stay put as at any
/// other time, the arriving workspace's moved as its pane is.
fn arriving(
    renderer: &mut GlesRenderer,
    pieces: &[(Framed, Point<i32, Logical>)],
    frame: &Frame,
    still: &impl Fn(&Framed) -> bool,
    size: Size<i32, Physical>,
    scale: f64,
) -> Vec<Elements> {
    let mut elements = Vec::new();
    for piece in pieces {
        let piece = std::slice::from_ref(piece);
        if still(&piece[0].0) {
            elements.extend(window_elements(renderer, piece, scale));
        } else {
            elements.extend(moved(renderer, piece, &frame.arriving, size, scale));
        }
    }
    elements
}

/// Windows drawn as `pane` has its workspace: moved by its offset, at its
/// alpha over their own, and cut to its part of the monitor.
fn moved(
    renderer: &mut GlesRenderer,
    pieces: &[(Framed, Point<i32, Logical>)],
    pane: &Pane,
    size: Size<i32, Physical>,
    scale: f64,
) -> Vec<Elements> {
    use smithay::backend::renderer::element::AsRenderElements;

    use crate::framed::FramedElement;

    let at_scale = smithay::utils::Scale::from(scale);
    let by = Point::from((snap(pane.offset.0, size.w), snap(pane.offset.1, size.h)));
    let clip = pixels(pane.clip, size);
    pieces
        .iter()
        .flat_map(|(window, at)| {
            let alpha = window.alpha() * pane.alpha;
            window.render_elements::<FramedElement<GlesRenderer>>(
                renderer,
                at.to_physical_precise_round(scale) + by,
                at_scale,
                alpha,
            )
        })
        .filter_map(|element| CropRenderElement::from_element(element, at_scale, clip))
        .map(Elements::Moving)
        .collect()
}

/// Part of a monitor `size` pixels big, in its pixels. Each edge is snapped
/// on its own, so two parts that meet in monitor sizes meet in pixels, with
/// no seam between and no row drawn twice.
fn pixels(area: Area, size: Size<i32, Physical>) -> Rectangle<i32, Physical> {
    let (left, top) = (snap(area.x, size.w), snap(area.y, size.h));
    let (right, bottom) = (snap(area.x + area.w, size.w), snap(area.y + area.h, size.h));
    Rectangle::new((left, top).into(), (right - left, bottom - top).into())
}

/// `share` of `side` pixels, to the nearest pixel, a half always up: so a
/// share one whole monitor on is always exactly `side` pixels on.
fn snap(share: f64, side: i32) -> i32 {
    (share * f64::from(side) + 0.5).floor() as i32
}

/// Something on this head is moving: draw it again at the next chance. A
/// frame queued brings a vblank, which draws again while the head is dirty.
/// A frame with nothing new in it, or one that could not be queued, brings
/// none, so a timer of one refresh stands in for it, one at a time.
pub(super) fn keep_moving(head: &mut Head, handle: &LoopHandle<'static, Compositor>) {
    head.dirty = true;
    if head.queued || head.ticking {
        return;
    }
    head.ticking = true;
    let crtc = head.crtc;
    let frame = frame_length(&head.output);
    let armed = handle.insert_source(Timer::from_duration(frame), move |_, (), state| {
        if let Running::Seat(session) = &mut state.backend
            && let Some(head) = session.heads.iter_mut().find(|head| head.crtc == crtc)
        {
            head.ticking = false;
            render(state, crtc);
        }
        TimeoutAction::Drop
    });
    if let Err(error) = armed {
        tracing::warn!(%error, "a transition's next frame was not timed");
        head.ticking = false;
    }
}

/// How long one of the monitor's frames lasts, at its refresh rate.
fn frame_length(output: &Output) -> Duration {
    match output.current_mode().map(|mode| mode.refresh) {
        Some(millihertz) if millihertz > 0 => {
            Duration::from_micros(1_000_000_000 / u64::from(millihertz.unsigned_abs()))
        }
        _ => FALLBACK_FRAME,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_hd() -> Size<i32, Physical> {
        Size::from((1920, 1080))
    }

    #[test]
    fn panes_a_monitor_apart_meet_with_no_seam() {
        for along in [0.0, 0.25, 0.5, 0.123_456, 0.999] {
            let old = pixels(
                Area {
                    x: -along,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                full_hd(),
            );
            let new = pixels(
                Area {
                    x: 1.0 - along,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                full_hd(),
            );
            assert_eq!(old.loc.x + old.size.w, new.loc.x, "{along}");
            assert_eq!(new.loc.x - old.loc.x, 1920, "{along}");
            assert_eq!(old.size.w, 1920);
        }
    }

    #[test]
    fn a_half_pixel_goes_the_same_way_on_both_sides() {
        assert_eq!(snap(-0.5 / 1920.0, 1920), 0);
        assert_eq!(snap(1.0 - 0.5 / 1920.0, 1920), 1920);
    }

    #[test]
    fn a_monitor_with_no_rate_gets_sixty_frames_a_second() {
        let output = Output::new(
            "test".to_owned(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
            },
        );
        assert_eq!(frame_length(&output), FALLBACK_FRAME, "no mode at all");
    }
}
