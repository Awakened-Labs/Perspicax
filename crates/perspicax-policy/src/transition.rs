//! Changing workspace, drawn: the old workspace giving way to the new one
//! over a moment, rather than in one frame, so a person sees which way they
//! went and that they wrapped.
//!
//! The switch itself is instant ([`crate::Workspaces`]): every window is
//! where it will be from the first frame, the facts say so, and a click lands
//! on what is there. A [`Transition`] only says what the seat draws while it
//! runs, moment by moment: which workspaces are seen, where, how see-through,
//! and what of the monitor each may be drawn on. It is written as a state
//! machine fed times from outside, as [`crate::EdgeDwell`] is, so it is
//! tested without a clock.
//!
//! Everything is in monitor sizes: an offset of `(1.0, 0.0)` is one monitor
//! to the right. So a transition across monitors of different sizes is drawn
//! by each at its own size.

use crate::{Cell, Direction, Grid};

/// How a switch is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effect {
    /// At once, as the switch is made.
    None,
    /// Both workspaces slide, the new one coming in from the side the person
    /// went towards.
    #[default]
    Slide,
    /// The old workspace's windows fade out as the new one's fade in.
    Crossfade,
    /// A black bar sweeps down the monitor, with the new workspace above it
    /// and what is left of the old one below.
    Wipe,
    /// Every other workspace slides past, the long way round, before the new
    /// one arrives.
    Carousel,
}

impl Effect {
    /// How long it takes when the config does not say, in milliseconds. A
    /// carousel has more to show than a slide, and a wipe crosses the whole
    /// monitor where a slide only finishes its move.
    #[must_use]
    pub fn usual_ms(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Slide | Self::Crossfade => 200,
            Self::Wipe => 300,
            Self::Carousel => 400,
        }
    }
}

/// How workspace switches are drawn: what a config's
/// `[workspaces.transition]` table says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transitioning {
    pub effect: Effect,
    /// How long one takes, in milliseconds, or the effect's own when `None`.
    pub duration_ms: Option<u64>,
}

impl Transitioning {
    /// How long one takes, in milliseconds.
    #[must_use]
    pub fn length_ms(&self) -> u64 {
        self.duration_ms.unwrap_or_else(|| self.effect.usual_ms())
    }
}

/// A wipe's bar, as a share of the monitor's height.
pub const WIPE_BAR: f64 = 1.0 / 12.0;

/// Part of a monitor, in monitor sizes: [`Area::WHOLE`] is all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    /// The whole monitor.
    pub const WHOLE: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };

    /// The whole monitor, moved by `offset`.
    fn moved(offset: (f64, f64)) -> Self {
        Self {
            x: offset.0,
            y: offset.1,
            ..Self::WHOLE
        }
    }
}

/// One workspace, as a moment of a transition shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pane {
    pub cell: Cell,
    /// How far its windows are drawn from where they are, in monitor sizes.
    pub offset: (f64, f64),
    /// How opaque its windows are drawn, over their own opacity.
    pub alpha: f32,
    /// The part of the monitor it may be drawn on. Nothing of it is drawn
    /// outside, so a window hanging off one workspace's edge never draws
    /// into the next.
    pub clip: Area,
}

/// A moment of a transition: the workspaces seen, and a wipe's bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The workspace switched to, whose windows are the ones on screen. Its
    /// offset may put it off the monitor, as a carousel's does until the
    /// end.
    pub arriving: Pane,
    /// Workspaces drawn over it, front to back: a crossfade's old one.
    pub over: Vec<Pane>,
    /// Workspaces drawn under it, front to back: the others. They never
    /// overlap it, so which is in front matters only to a window that is in
    /// neither, a sticky one say, which is drawn over them all.
    pub under: Vec<Pane>,
    /// A wipe's bar, drawn over the windows and under the panels.
    pub bar: Option<Area>,
}

/// A switch, being drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    effect: Effect,
    /// Every workspace seen on the way, the first and the last included. Two,
    /// but for a carousel, which passes them all.
    path: Vec<Cell>,
    /// The way the person went, if the switch had one.
    heading: Option<Direction>,
    /// The side each workspace on the path comes in from, per axis: -1, 0 or
    /// 1, with x to the right and y down.
    toward: (i8, i8),
    since: u64,
    length: u64,
}

impl Transition {
    /// Start drawing a switch from `from` to `to` at `now`, going `heading`
    /// if the switch had a direction: a key, an edge. `None` when there is
    /// nothing to draw: no effect, no length, nowhere to go.
    ///
    /// A slide comes in from the side the person went towards, across a wrap
    /// too: right from the last column slides right, which is the point of a
    /// grid that wraps. A switch with no direction, by number, a pager, a
    /// scroll, slides along the line from the one cell to the other on the
    /// grid, one monitor's width or height on each axis it crosses.
    ///
    /// A carousel goes round every workspace in reading order, whatever the
    /// grid's shape, the long way: against the direction of the short way,
    /// so each switch passes all the others. Back, when both ways are as
    /// long.
    #[must_use]
    pub fn new(
        how: Transitioning,
        grid: Grid,
        from: Cell,
        to: Cell,
        heading: Option<Direction>,
        now: u64,
    ) -> Option<Self> {
        let length = how.length_ms();
        let len = grid.len();
        if how.effect == Effect::None || length == 0 || from == to || from.0 >= len || to.0 >= len {
            return None;
        }
        let (path, toward) = match how.effect {
            Effect::Carousel => the_long_way(len, from, to),
            _ => (vec![from, to], toward(grid, from, to, heading)),
        };
        Some(Self {
            effect: how.effect,
            path,
            heading,
            toward,
            since: now,
            length,
        })
    }

    /// The workspace switched from.
    #[must_use]
    pub fn from(&self) -> Cell {
        self.path[0]
    }

    /// The workspace switched to.
    #[must_use]
    pub fn to(&self) -> Cell {
        self.path[self.path.len() - 1]
    }

    /// The way the person went, if the switch had one.
    #[must_use]
    pub fn heading(&self) -> Option<Direction> {
        self.heading
    }

    /// What to draw at `now`, or `None` once it is over and the settled
    /// workspace is drawn as it is.
    #[must_use]
    pub fn at(&self, now: u64) -> Option<Frame> {
        let gone = now.saturating_sub(self.since);
        if gone >= self.length {
            return None;
        }
        let progress = gone as f64 / self.length as f64;
        match self.effect {
            Effect::None => None,
            Effect::Slide => Some(self.strip(ease_out(progress))),
            Effect::Carousel => Some(self.strip(ease_in_out(progress))),
            Effect::Crossfade => Some(self.crossfade(smoothstep(progress))),
            Effect::Wipe => Some(self.wipe(ease_in_out(progress))),
        }
    }

    /// The path laid out in a strip, each workspace one monitor on from the
    /// last, `along` of the way down it. Only the workspaces that overlap
    /// the monitor are drawn, and the arriving one, which the frame always
    /// has.
    fn strip(&self, eased: f64) -> Frame {
        let last = self.path.len() - 1;
        let along = eased * last as f64;
        let (x, y) = (f64::from(self.toward.0), f64::from(self.toward.1));
        let pane = |index: usize| {
            let by = index as f64 - along;
            let offset = (by * x, by * y);
            Pane {
                cell: self.path[index],
                offset,
                alpha: 1.0,
                clip: Area::moved(offset),
            }
        };
        Frame {
            arriving: pane(last),
            over: Vec::new(),
            under: (0..last)
                .filter(|&index| (index as f64 - along).abs() < 1.0)
                .map(pane)
                .collect(),
            bar: None,
        }
    }

    /// The old workspace in front, fading out, and the new one fading in.
    fn crossfade(&self, eased: f64) -> Frame {
        let pane = |cell, alpha: f64| Pane {
            cell,
            offset: (0.0, 0.0),
            alpha: alpha as f32,
            clip: Area::WHOLE,
        };
        Frame {
            arriving: pane(self.to(), eased),
            over: vec![pane(self.from(), 1.0 - eased)],
            under: Vec::new(),
            bar: None,
        }
    }

    /// The bar's top going from just above the monitor to its bottom edge,
    /// the new workspace above it and the old one below.
    fn wipe(&self, eased: f64) -> Frame {
        let top = -WIPE_BAR + eased * (1.0 + WIPE_BAR);
        let below = top + WIPE_BAR;
        let pane = |cell, y: f64, h: f64| Pane {
            cell,
            offset: (0.0, 0.0),
            alpha: 1.0,
            clip: Area {
                y,
                h: h.max(0.0),
                ..Area::WHOLE
            },
        };
        Frame {
            arriving: pane(self.to(), 0.0, top),
            over: Vec::new(),
            under: vec![pane(self.from(), below, 1.0 - below)],
            bar: Some(Area {
                y: top,
                h: WIPE_BAR,
                ..Area::WHOLE
            }),
        }
    }
}

/// The side a slide comes in from: the way the person went, or along the
/// line from one cell to the other, unwrapped, one step on each axis it
/// crosses.
fn toward(grid: Grid, from: Cell, to: Cell, heading: Option<Direction>) -> (i8, i8) {
    let sign = |from: u16, to: u16| match to.cmp(&from) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    match heading {
        Some(Direction::Left) => (-1, 0),
        Some(Direction::Right) => (1, 0),
        Some(Direction::Up) => (0, -1),
        Some(Direction::Down) => (0, 1),
        None => {
            let columns = grid.columns.max(1);
            (
                sign(from.0 % columns, to.0 % columns),
                sign(from.0 / columns, to.0 / columns),
            )
        }
    }
}

/// Round `len` workspaces in reading order from `from` to `to` the long way,
/// and which side each comes in from: from the left going back, from the
/// right going forward.
fn the_long_way(len: u16, from: Cell, to: Cell) -> (Vec<Cell>, (i8, i8)) {
    let len = u32::from(len);
    let (from, to) = (u32::from(from.0), u32::from(to.0));
    let forward = (to + len - from) % len;
    let back = len - forward;
    let (steps, step) = if forward <= back {
        (back, len - 1)
    } else {
        (forward, 1)
    };
    let path = (0..=steps)
        .map(|taken| Cell(((from + taken * step) % len) as u16))
        .collect();
    let toward = if step == 1 { (1, 0) } else { (-1, 0) };
    (path, toward)
}

/// A value on its way from one place to another, eased out: how much of a
/// window is still drawn as it rolls up, or how lit a titlebar button is as
/// its light comes up. Fed times from outside, as a [`Transition`] is, and
/// in milliseconds from any origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    from: f64,
    to: f64,
    since_ms: u64,
    length_ms: u64,
}

impl Tween {
    /// From `from` to `to`, setting off at `now_ms`, `length_ms` long for
    /// the whole way from 0 to 1: a shorter way takes as much less, so a
    /// move turned round midway goes back at the pace it came.
    #[must_use]
    pub fn new(from: f64, to: f64, now_ms: u64, length_ms: u64) -> Self {
        let share = (to - from).abs().min(1.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "a length of at most a few seconds, times a share of at most one"
        )]
        let length_ms = (length_ms as f64 * share).round() as u64;
        Self {
            from,
            to,
            since_ms: now_ms,
            length_ms,
        }
    }

    /// Where it is at `now_ms`, or `None` once it has arrived.
    #[must_use]
    pub fn at(&self, now_ms: u64) -> Option<f64> {
        let gone = now_ms.saturating_sub(self.since_ms);
        if gone >= self.length_ms {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "milliseconds of a move a few seconds long at most"
        )]
        let t = gone as f64 / self.length_ms as f64;
        Some(self.from + (self.to - self.from) * ease_out(t))
    }

    /// Where it is at `now_ms`, arrived or not: its end, once there.
    #[must_use]
    pub fn value(&self, now_ms: u64) -> f64 {
        self.at(now_ms).unwrap_or(self.to)
    }

    /// Where it is going.
    #[must_use]
    pub fn to(&self) -> f64 {
        self.to
    }
}

/// Fast, then settling: a move that arrives.
#[must_use]
pub fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Slow, fast, slow: a move that sets off and arrives.
#[must_use]
pub fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t.powi(3)
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Smooth at both ends, gentler than [`ease_in_out`]: a fade.
#[must_use]
pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eased out from where it set off, it arrives on time, and stays.
    #[test]
    fn a_tween_eases_from_where_it_started_and_ends_on_time() {
        let tween = Tween::new(1.0, 0.0, 1000, 150);
        assert_eq!(tween.at(1000), Some(1.0), "where it set off");
        let halfway = tween.at(1075).unwrap();
        assert!(
            halfway < 0.5,
            "eased out: past halfway at half the time, {halfway}"
        );
        assert!(tween.at(1100).unwrap() < halfway, "and on");
        assert_eq!(tween.at(1150), None, "there, on time");
        assert!(tween.value(5000).abs() < f64::EPSILON, "and staying");
        assert_eq!(
            tween.at(500),
            Some(1.0),
            "a clock before it set off is its start"
        );
        assert_eq!(
            Tween::new(0.0, 1.0, 0, 0).at(0),
            None,
            "no time at all is there at once"
        );
    }

    /// A roll turned round midway goes back from where it was, not from
    /// where it would have ended, and takes as long as it took to get there.
    #[test]
    fn a_roll_turned_round_starts_from_where_it_was() {
        let up = Tween::new(1.0, 0.0, 0, 200);
        let there = up.value(60);
        let down = Tween::new(there, 1.0, 60, 200);
        assert!(
            (down.at(60).unwrap() - there).abs() < f64::EPSILON,
            "from where it was"
        );
        let back = (200.0 * (1.0 - there)).round() as u64;
        assert!(
            down.at(60 + back - 1).is_some() && down.at(60 + back).is_none(),
            "{back} ms"
        );
        assert!((down.to() - 1.0).abs() < f64::EPSILON);
    }

    const TWO_BY_TWO: Grid = Grid {
        columns: 2,
        rows: 2,
        wrap: true,
    };

    const ROW_OF_FOUR: Grid = Grid {
        columns: 4,
        rows: 1,
        wrap: false,
    };

    fn effect(effect: Effect) -> Transitioning {
        Transitioning {
            effect,
            duration_ms: None,
        }
    }

    fn slide(grid: Grid, from: u16, to: u16, heading: Option<Direction>) -> Transition {
        Transition::new(
            effect(Effect::Slide),
            grid,
            Cell(from),
            Cell(to),
            heading,
            0,
        )
        .expect("a slide")
    }

    fn carousel(from: u16, to: u16) -> Vec<Cell> {
        Transition::new(
            effect(Effect::Carousel),
            TWO_BY_TWO,
            Cell(from),
            Cell(to),
            None,
            0,
        )
        .expect("a carousel")
        .path
    }

    fn cells(path: &[u16]) -> Vec<Cell> {
        path.iter().copied().map(Cell).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn no_effect_draws_no_transition() {
        assert_eq!(
            Transition::new(effect(Effect::None), TWO_BY_TWO, Cell(0), Cell(1), None, 0),
            None
        );
    }

    #[test]
    fn a_switch_to_where_it_is_draws_none() {
        let how = effect(Effect::Slide);
        assert_eq!(
            Transition::new(how, TWO_BY_TWO, Cell(2), Cell(2), None, 0),
            None
        );
        assert_eq!(
            Transition::new(how, Grid::default(), Cell(0), Cell(1), None, 0),
            None,
            "a grid of one has nowhere to go"
        );
    }

    #[test]
    fn a_slide_comes_in_from_the_side_it_was_asked_for() {
        let start = slide(ROW_OF_FOUR, 1, 2, Some(Direction::Right))
            .at(0)
            .expect("running");
        assert_eq!(start.arriving.cell, Cell(2));
        assert_eq!(start.arriving.offset, (1.0, 0.0), "one monitor right");
        assert_eq!(start.under.len(), 1);
        assert_eq!(start.under[0].cell, Cell(1));
        assert_eq!(start.under[0].offset, (0.0, 0.0), "where it was");
        let down = slide(TWO_BY_TWO, 0, 2, Some(Direction::Down))
            .at(0)
            .expect("running");
        assert_eq!(down.arriving.offset, (0.0, 1.0), "from below");

        let midway = slide(ROW_OF_FOUR, 1, 2, Some(Direction::Right))
            .at(100)
            .expect("running");
        let x = midway.arriving.offset.0;
        assert!(0.0 < x && x < 0.5, "eased out, past halfway: {x}");
        assert!(
            close(midway.under[0].offset.0, x - 1.0),
            "the old one beside it, edge to edge"
        );
        assert_eq!(
            midway.arriving.clip,
            Area::moved(midway.arriving.offset),
            "each clipped to its own place"
        );
    }

    #[test]
    fn right_across_a_wrap_still_slides_right() {
        let wrapped = slide(TWO_BY_TWO, 1, 0, Some(Direction::Right));
        assert_eq!(wrapped.toward, (1, 0));
        assert_eq!(wrapped.at(0).expect("running").arriving.offset, (1.0, 0.0));
    }

    #[test]
    fn a_jump_slides_one_monitor_along_the_grid() {
        assert_eq!(
            slide(ROW_OF_FOUR, 0, 3, None).toward,
            (1, 0),
            "one monitor, not three"
        );
        assert_eq!(slide(TWO_BY_TWO, 0, 3, None).toward, (1, 1), "diagonal");
        assert_eq!(slide(TWO_BY_TWO, 3, 0, None).toward, (-1, -1));
        assert_eq!(
            slide(TWO_BY_TWO, 1, 2, None).toward,
            (-1, 1),
            "a scroll from the end of a row to the start of the next"
        );
    }

    #[test]
    fn the_carousel_goes_the_long_way_round() {
        // Workspaces 2 -> 3, as a person counts them, passes 1 and 4.
        assert_eq!(carousel(1, 2), cells(&[1, 0, 3, 2]));
        assert_eq!(carousel(2, 1), cells(&[2, 3, 0, 1]), "and back, forward");
        assert_eq!(carousel(3, 0), cells(&[3, 2, 1, 0]), "4 -> 1 too");
        assert_eq!(carousel(0, 2), cells(&[0, 3, 2]), "back, when as long");
    }

    #[test]
    fn the_carousel_comes_in_from_the_side_it_goes() {
        let back = Transition::new(
            effect(Effect::Carousel),
            TWO_BY_TWO,
            Cell(1),
            Cell(2),
            None,
            0,
        )
        .expect("a carousel");
        assert_eq!(back.toward, (-1, 0), "going back, from the left");
        let forward = Transition::new(
            effect(Effect::Carousel),
            TWO_BY_TWO,
            Cell(2),
            Cell(1),
            None,
            0,
        )
        .expect("a carousel");
        assert_eq!(forward.toward, (1, 0));
        assert_eq!(
            forward.at(0).expect("running").arriving.offset,
            (3.0, 0.0),
            "three monitors off, until the end"
        );
    }

    #[test]
    fn the_carousel_shows_at_most_two_workspaces_at_once() {
        let carousel = Transition::new(
            effect(Effect::Carousel),
            TWO_BY_TWO,
            Cell(1),
            Cell(2),
            None,
            0,
        )
        .expect("a carousel");
        let mut seen = Vec::new();
        for now in 0..400 {
            let frame = carousel.at(now).expect("running");
            let on_screen: Vec<_> = std::iter::once(frame.arriving)
                .chain(frame.under)
                .filter(|pane| pane.offset.0.abs() < 1.0)
                .collect();
            assert!((1..=2).contains(&on_screen.len()), "{now}: {on_screen:?}");
            for pane in on_screen {
                if !seen.contains(&pane.cell) {
                    seen.push(pane.cell);
                }
            }
        }
        assert_eq!(seen, cells(&[1, 0, 3, 2]), "each, in turn");
    }

    #[test]
    fn a_wipe_bar_runs_from_above_the_top_to_below_the_bottom() {
        let wipe = Transition::new(
            effect(Effect::Wipe),
            TWO_BY_TWO,
            Cell(0),
            Cell(1),
            None,
            1000,
        )
        .expect("a wipe");
        let bar = |now| wipe.at(now).expect("running").bar.expect("a bar");
        assert_eq!(bar(1000).y, -WIPE_BAR, "just above the top");
        assert_eq!(bar(1000).h, WIPE_BAR);
        assert!(bar(1150).y > 0.0 && bar(1150).y < 1.0);
        assert!(bar(1299).y > 1.0 - WIPE_BAR, "nearly off the bottom");
        assert!(bar(1100).y < bar(1200).y, "down, never up");
    }

    #[test]
    fn the_new_workspace_is_above_the_bar_and_the_old_below() {
        let wipe = Transition::new(effect(Effect::Wipe), TWO_BY_TWO, Cell(0), Cell(1), None, 0)
            .expect("a wipe");
        let start = wipe.at(0).expect("running");
        assert_eq!(start.arriving.clip.h, 0.0, "nothing of the new one yet");
        assert_eq!(start.under[0].clip, Area::WHOLE, "all of the old one");
        let midway = wipe.at(150).expect("running");
        let bar = midway.bar.expect("a bar");
        assert_eq!(midway.arriving.cell, Cell(1));
        assert_eq!(midway.arriving.clip.y, 0.0);
        assert_eq!(midway.arriving.clip.h, bar.y, "down to the bar");
        assert_eq!(midway.under[0].cell, Cell(0));
        assert_eq!(midway.under[0].clip.y, bar.y + bar.h, "from under it");
        assert!(close(midway.under[0].clip.y + midway.under[0].clip.h, 1.0));
        assert_eq!(midway.arriving.offset, (0.0, 0.0), "nothing moves");
    }

    #[test]
    fn a_crossfade_puts_the_old_workspace_in_front_and_trades_alpha() {
        let fade = Transition::new(
            effect(Effect::Crossfade),
            TWO_BY_TWO,
            Cell(0),
            Cell(3),
            None,
            0,
        )
        .expect("a crossfade");
        let start = fade.at(0).expect("running");
        assert_eq!(start.over.len(), 1);
        assert_eq!(start.over[0].cell, Cell(0), "the old one, in front");
        assert_eq!(start.over[0].alpha, 1.0);
        assert_eq!(start.arriving.alpha, 0.0);
        assert!(start.under.is_empty());
        let midway = fade.at(100).expect("running");
        assert_eq!(midway.arriving.alpha + midway.over[0].alpha, 1.0);
        assert_eq!(midway.arriving.alpha, 0.5, "smooth, and even");
        assert_eq!(midway.arriving.offset, (0.0, 0.0));
    }

    #[test]
    fn a_transition_is_over_once_its_length_has_passed() {
        let slide = Transition::new(
            effect(Effect::Slide),
            TWO_BY_TWO,
            Cell(0),
            Cell(1),
            Some(Direction::Right),
            5000,
        )
        .expect("a slide");
        assert!(slide.at(5000).is_some());
        assert!(slide.at(5199).is_some());
        assert_eq!(slide.at(5200), None);
        assert!(
            slide.at(4000).is_some(),
            "a clock read before it began is its start"
        );
        assert_eq!(slide.from(), Cell(0));
        assert_eq!(slide.to(), Cell(1));
        assert_eq!(slide.heading(), Some(Direction::Right));
    }

    #[test]
    fn an_effect_takes_its_usual_length_unless_given_one() {
        assert_eq!(effect(Effect::Slide).length_ms(), 200);
        assert_eq!(effect(Effect::Carousel).length_ms(), 400);
        let given = Transitioning {
            effect: Effect::Carousel,
            duration_ms: Some(900),
        };
        assert_eq!(given.length_ms(), 900);
        let carousel =
            Transition::new(given, TWO_BY_TWO, Cell(0), Cell(1), None, 0).expect("a carousel");
        assert!(carousel.at(899).is_some());
        assert_eq!(carousel.at(900), None);
        assert_eq!(
            Transitioning::default(),
            effect(Effect::Slide),
            "a slide, at its own length"
        );
    }

    #[test]
    fn every_easing_starts_at_none_and_ends_at_all() {
        for ease in [ease_out, ease_in_out, smoothstep] {
            assert_eq!(ease(0.0), 0.0);
            assert_eq!(ease(1.0), 1.0);
            assert_eq!(ease(2.0), 1.0, "clamped");
            assert!(ease(0.25) < ease(0.75), "onwards");
        }
    }
}
