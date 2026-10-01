//! Changing workspace with the pointer: pushing against an edge of the desk,
//! or scrolling over the desktop.
//!
//! Only the *outer* edges flip. Between two monitors side by side the pointer
//! crosses over, as a person expects; the edge flips only where there is no
//! monitor beyond it. That includes the strip of a taller monitor's side that
//! a shorter neighbour does not reach, because the pointer can be pushed
//! against it just as hard.
//!
//! The pointer has to rest against an edge for a moment before anything
//! happens, so that throwing it at a corner to reach a menu does not change
//! workspace. [`EdgeDwell`] is that moment, written as a state machine fed
//! times from outside, so it is tested without a clock.

use crate::{Direction, Rect};

/// Which of the ways to change workspace with the pointer are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flipping {
    /// Resting the pointer against an outer edge of the desk flips that way.
    pub edge: bool,
    /// How long it has to rest there, in milliseconds.
    pub delay_ms: u64,
    /// Resting there while dragging a window flips too, and takes the window.
    /// Off, a drag never flips.
    pub while_dragging: bool,
    /// Scrolling over the desktop (no window under the pointer) flips: down
    /// and right to the next workspace, up and left to the previous.
    pub scroll: bool,
}

impl Default for Flipping {
    /// All off, with Enlightenment's delay for when edge flipping is turned
    /// on without one.
    fn default() -> Self {
        Self {
            edge: false,
            delay_ms: 300,
            while_dragging: false,
            scroll: false,
        }
    }
}

/// Which outer edge of the desk `point` is pressed against, if any: on the
/// last pixel of an output's side, where no other output continues.
///
/// The pointer's far edges are exclusive (a 1920-wide output's pixels run
/// 0 to 1919.99), so "on the right edge" is anywhere in its last pixel.
#[must_use]
pub fn edge_at(point: (f64, f64), outputs: &[Rect]) -> Option<Direction> {
    let (x, y) = point;
    let output = outputs.iter().find(|o| {
        x >= f64::from(o.x)
            && x < f64::from(o.x + o.w)
            && y >= f64::from(o.y)
            && y < f64::from(o.y + o.h)
    })?;
    // One pixel past the edge, on the far side: is that on some output?
    let beyond = |px: f64, py: f64| {
        outputs.iter().any(|o| {
            px >= f64::from(o.x)
                && px < f64::from(o.x + o.w)
                && py >= f64::from(o.y)
                && py < f64::from(o.y + o.h)
        })
    };
    let (left, top) = (f64::from(output.x), f64::from(output.y));
    let (right, bottom) = (
        f64::from(output.x + output.w),
        f64::from(output.y + output.h),
    );
    [
        (x < left + 1.0, Direction::Left, (left - 1.0, y)),
        (x >= right - 1.0, Direction::Right, (right, y)),
        (y < top + 1.0, Direction::Up, (x, top - 1.0)),
        (y >= bottom - 1.0, Direction::Down, (x, bottom)),
    ]
    .into_iter()
    .find(|&(on, _, (px, py))| on && !beyond(px, py))
    .map(|(_, direction, _)| direction)
}

/// Where to put the pointer after flipping `direction`: just inside the
/// opposite outer edge of the desk, on the same line, as if the desk were a
/// loop. Not on the edge itself, or resting there would flip straight back.
#[must_use]
pub fn arrival(point: (f64, f64), direction: Direction, outputs: &[Rect]) -> (f64, f64) {
    /// Far enough in to be off the edge, near enough to look continuous.
    const INSET: f64 = 8.0;
    let (x, y) = point;
    let on_row = |o: &&Rect| y >= f64::from(o.y) && y < f64::from(o.y + o.h);
    let on_column = |o: &&Rect| x >= f64::from(o.x) && x < f64::from(o.x + o.w);
    let landing = match direction {
        Direction::Right => outputs
            .iter()
            .filter(on_row)
            .map(|o| f64::from(o.x))
            .reduce(f64::min)
            .map(|left| (left + INSET, y)),
        Direction::Left => outputs
            .iter()
            .filter(on_row)
            .map(|o| f64::from(o.x + o.w))
            .reduce(f64::max)
            .map(|right| (right - INSET, y)),
        Direction::Down => outputs
            .iter()
            .filter(on_column)
            .map(|o| f64::from(o.y))
            .reduce(f64::min)
            .map(|top| (x, top + INSET)),
        Direction::Up => outputs
            .iter()
            .filter(on_column)
            .map(|o| f64::from(o.y + o.h))
            .reduce(f64::max)
            .map(|bottom| (x, bottom - INSET)),
    };
    landing.unwrap_or(point)
}

/// The pointer resting against an edge, and whether it has rested long
/// enough.
///
/// Fed the edge the pointer is at (or `None`) with the time, in
/// milliseconds from any fixed origin, whenever the pointer moves and when a
/// timer the caller armed for [`EdgeDwell::due`] goes off. It flips once per
/// arrival at an edge: to flip again, leave the edge and come back, which the
/// pointer does anyway when [`arrival`] moves it across the desk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeDwell {
    delay: u64,
    /// The edge being rested against, and since when.
    resting: Option<(Direction, u64)>,
    /// Already flipped for this rest.
    spent: bool,
}

impl EdgeDwell {
    /// A dwell of `delay` milliseconds.
    #[must_use]
    pub fn new(delay: u64) -> Self {
        Self {
            delay,
            resting: None,
            spent: false,
        }
    }

    /// The pointer is at `edge` now. `Some` when it has rested there long
    /// enough to flip, once.
    pub fn feed(&mut self, edge: Option<Direction>, now: u64) -> Option<Direction> {
        match (edge, self.resting) {
            (None, _) => {
                self.resting = None;
                self.spent = false;
                None
            }
            (Some(edge), Some((resting, since))) if edge == resting => {
                if !self.spent && now.saturating_sub(since) >= self.delay {
                    self.spent = true;
                    Some(edge)
                } else {
                    None
                }
            }
            (Some(edge), _) => {
                self.resting = Some((edge, now));
                self.spent = false;
                (self.delay == 0).then(|| {
                    self.spent = true;
                    edge
                })
            }
        }
    }

    /// When to check again, if the pointer stays where it is: the moment the
    /// rest becomes long enough. `None` when there is nothing to wait for.
    #[must_use]
    pub fn due(&self) -> Option<u64> {
        match self.resting {
            Some((_, since)) if !self.spent => Some(since + self.delay),
            _ => None,
        }
    }

    /// Forget the rest, as when a button goes down: a click at the edge of
    /// the screen is a click, not a flip.
    pub fn cancel(&mut self) {
        self.resting = None;
        self.spent = false;
    }
}

/// Scroll, counted out in notches of a wheel. A wheel reports 120 a notch; a
/// touchpad reports pixels, and [`NOTCH_PIXELS`] of them count as one, so one
/// lazy swipe does not race through every workspace.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Notches {
    gathered: f64,
}

/// How far a touchpad scrolls for one notch: a decisive swipe, not a twitch.
pub const NOTCH_PIXELS: f64 = 60.0;

impl Notches {
    /// Add one scroll event: `v120` from a wheel when it gave one, otherwise
    /// `pixels`. Returns whole notches, positive downward, and keeps the rest
    /// for next time.
    pub fn feed(&mut self, v120: Option<f64>, pixels: f64) -> i32 {
        self.gathered += match v120 {
            Some(v120) => v120 / 120.0,
            None => pixels / NOTCH_PIXELS,
        };
        let whole = self.gathered.trunc();
        self.gathered -= whole;
        // A handful of notches at most, whatever a device reports.
        whole.clamp(-16.0, 16.0) as i32
    }

    /// Start counting afresh, as when a touchpad finger lifts.
    pub fn reset(&mut self) {
        self.gathered = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2560x1440 monitor with a 1920x1080 one to its right, 180 down.
    fn pair() -> [Rect; 2] {
        [
            Rect::new(0, 0, 2560, 1440),
            Rect::new(2560, 180, 1920, 1080),
        ]
    }

    #[test]
    fn the_far_sides_of_the_desk_are_edges_and_the_seam_between_monitors_is_not() {
        let desk = pair();
        assert_eq!(edge_at((0.0, 700.0), &desk), Some(Direction::Left));
        assert_eq!(edge_at((4479.5, 700.0), &desk), Some(Direction::Right));
        assert_eq!(edge_at((2559.5, 700.0), &desk), None, "the seam");
        assert_eq!(
            edge_at((2560.0, 700.0), &desk),
            None,
            "the seam, from the other side"
        );
    }

    #[test]
    fn the_strip_of_the_taller_monitor_beyond_the_shorter_one_is_an_edge() {
        let desk = pair();
        assert_eq!(
            edge_at((2559.5, 100.0), &desk),
            Some(Direction::Right),
            "above where the right monitor starts"
        );
        assert_eq!(
            edge_at((2559.5, 1300.0), &desk),
            Some(Direction::Right),
            "below it"
        );
    }

    #[test]
    fn the_top_and_bottom_of_a_row_of_monitors_are_edges() {
        let desk = pair();
        assert_eq!(edge_at((3000.0, 180.0), &desk), Some(Direction::Up));
        assert_eq!(edge_at((3000.0, 1259.5), &desk), Some(Direction::Down));
        assert_eq!(edge_at((1000.0, 1439.5), &desk), Some(Direction::Down));
    }

    #[test]
    fn the_middle_of_a_monitor_is_no_edge() {
        assert_eq!(edge_at((500.0, 500.0), &pair()), None);
    }

    #[test]
    fn flipping_right_arrives_just_inside_the_left_of_the_desk() {
        let desk = pair();
        assert_eq!(
            arrival((4479.5, 700.0), Direction::Right, &desk),
            (8.0, 700.0)
        );
        assert_eq!(
            arrival((0.0, 700.0), Direction::Left, &desk),
            (4472.0, 700.0),
            "the right monitor reaches this height"
        );
        assert_eq!(
            arrival((0.0, 100.0), Direction::Left, &desk),
            (2552.0, 100.0),
            "it does not reach this one; the left monitor's own edge"
        );
    }

    #[test]
    fn a_rest_flips_once_the_delay_has_passed_and_only_once() {
        let mut dwell = EdgeDwell::new(300);
        assert_eq!(dwell.feed(Some(Direction::Right), 1000), None);
        assert_eq!(dwell.due(), Some(1300));
        assert_eq!(dwell.feed(Some(Direction::Right), 1200), None);
        assert_eq!(
            dwell.feed(Some(Direction::Right), 1300),
            Some(Direction::Right)
        );
        assert_eq!(dwell.feed(Some(Direction::Right), 2000), None, "spent");
        assert_eq!(dwell.due(), None);
    }

    #[test]
    fn leaving_the_edge_starts_the_rest_over() {
        let mut dwell = EdgeDwell::new(300);
        dwell.feed(Some(Direction::Right), 1000);
        dwell.feed(None, 1200);
        assert_eq!(dwell.feed(Some(Direction::Right), 1250), None);
        assert_eq!(
            dwell.feed(Some(Direction::Right), 1500),
            None,
            "250 ms, not 500"
        );
        assert_eq!(
            dwell.feed(Some(Direction::Right), 1550),
            Some(Direction::Right)
        );
    }

    #[test]
    fn sliding_into_a_corner_starts_the_rest_over_on_the_new_edge() {
        let mut dwell = EdgeDwell::new(300);
        dwell.feed(Some(Direction::Right), 1000);
        assert_eq!(dwell.feed(Some(Direction::Down), 1200), None);
        assert_eq!(
            dwell.feed(Some(Direction::Down), 1500),
            Some(Direction::Down)
        );
    }

    #[test]
    fn a_click_cancels_the_rest() {
        let mut dwell = EdgeDwell::new(300);
        dwell.feed(Some(Direction::Left), 1000);
        dwell.cancel();
        assert_eq!(dwell.due(), None);
        assert_eq!(
            dwell.feed(Some(Direction::Left), 1400),
            None,
            "a fresh rest"
        );
    }

    #[test]
    fn no_delay_flips_on_arrival() {
        let mut dwell = EdgeDwell::new(0);
        assert_eq!(dwell.feed(Some(Direction::Up), 5), Some(Direction::Up));
        assert_eq!(dwell.feed(Some(Direction::Up), 6), None);
    }

    #[test]
    fn a_wheel_notch_is_one_and_a_touchpad_gathers_pixels() {
        let mut wheel = Notches::default();
        assert_eq!(wheel.feed(Some(120.0), 15.0), 1);
        assert_eq!(wheel.feed(Some(-240.0), -30.0), -2);
        let mut pad = Notches::default();
        assert_eq!(pad.feed(None, 25.0), 0);
        assert_eq!(pad.feed(None, 25.0), 0);
        assert_eq!(pad.feed(None, 25.0), 1, "75 pixels: one notch, 15 kept");
        pad.reset();
        assert_eq!(pad.feed(None, 50.0), 0, "the 15 were dropped");
    }
}
