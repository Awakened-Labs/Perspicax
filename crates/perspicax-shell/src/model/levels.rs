//! The mixer's moving bars, as they are drawn between one reading of the
//! spectrum and the next.
//!
//! A bar jumps up to what is heard, and falls back at [`FALL`] a second, so
//! that a beat shows as a jump and a slow fall rather than a flicker. Above
//! each bar a cap marks the highest it reached; it waits [`HOLD`], then
//! falls at [`CAP_FALL`] a second, slower than the bar, never below it. All
//! of it at rest, with nothing heard, asks for no more drawing.

use std::time::Duration;

use crate::audio::BANDS;

/// How fast a bar falls, and a cap, in heights a second.
const FALL: f32 = 2.0;
const CAP_FALL: f32 = 0.8;
/// How long a cap stays where a bar pushed it before it falls.
const HOLD: Duration = Duration::from_millis(400);

/// The bars, as drawn now.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Bars {
    bands: [Band; BANDS],
}

/// One bar, its cap, and what was last heard of its band, each from 0 to
/// 1.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Band {
    heard: f32,
    height: f32,
    cap: f32,
    /// How long its cap has waited where the bar pushed it.
    held: Duration,
}

impl Bars {
    /// What is heard now: each bar jumps up to its band, and its cap with
    /// it.
    pub(crate) fn hear(&mut self, heard: [f32; BANDS]) {
        for (band, heard) in self.bands.iter_mut().zip(heard) {
            band.heard = heard;
            band.height = band.height.max(heard);
            if band.height >= band.cap {
                band.cap = band.height;
                band.held = Duration::ZERO;
            }
        }
    }

    /// Let `since` pass: each bar falls towards what was heard, and each cap
    /// that has waited long enough falls towards its bar. Whether anything
    /// is still to move.
    pub(crate) fn step(&mut self, since: Duration) -> bool {
        let seconds = since.as_secs_f32();
        for band in &mut self.bands {
            band.height = (band.height - FALL * seconds).max(band.heard);
            band.held += since;
            if band.held >= HOLD {
                band.cap = (band.cap - CAP_FALL * seconds).max(band.height);
            }
        }
        self.moving()
    }

    /// Whether a bar is above what was heard, or a cap above its bar.
    pub(crate) fn moving(&self) -> bool {
        self.bands
            .iter()
            .any(|band| band.height > band.heard || band.cap > band.height)
    }

    /// Each bar's height and its cap's, from the deepest band.
    pub(crate) fn shown(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.bands.iter().map(|band| (band.height, band.cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fourth bar's height and cap.
    fn fourth(bars: &Bars) -> (f32, f32) {
        bars.shown().nth(3).expect("sixteen")
    }

    fn one(level: f32) -> [f32; BANDS] {
        let mut heard = [0.0; BANDS];
        heard[3] = level;
        heard
    }

    #[test]
    fn a_bar_jumps_up_and_falls_steadily_and_its_cap_waits_then_falls_slower() {
        let mut bars = Bars::default();
        bars.hear(one(0.9));
        assert_eq!(fourth(&bars), (0.9, 0.9), "up at once");
        bars.hear(one(0.0));
        assert!(bars.step(Duration::from_millis(100)));
        let (height, cap) = fourth(&bars);
        assert!((height - 0.7).abs() < 1e-6, "{height}");
        assert_eq!(cap, 0.9, "the cap waits");
        bars.step(Duration::from_millis(300));
        let (height, cap) = fourth(&bars);
        assert!(height < 0.2);
        assert!(cap < 0.9 && cap > height, "falling, slower: {cap}");
    }

    #[test]
    fn a_bar_falls_no_lower_than_what_is_heard() {
        let mut bars = Bars::default();
        bars.hear(one(0.9));
        bars.hear(one(0.5));
        bars.step(Duration::from_secs(1));
        assert_eq!(fourth(&bars).0, 0.5);
    }

    #[test]
    fn bars_at_rest_ask_for_no_frame() {
        let mut bars = Bars::default();
        assert!(!bars.moving(), "nothing heard");
        bars.hear(one(0.6));
        bars.hear(one(0.0));
        let mut steps = 0;
        while bars.step(Duration::from_millis(16)) {
            steps += 1;
            assert!(steps < 200, "it comes to rest");
        }
        assert_eq!(fourth(&bars), (0.0, 0.0));
        assert!(!bars.step(Duration::from_millis(16)));
    }
}
