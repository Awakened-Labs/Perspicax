//! Monitors as a display tool sees them, and whether what it asks for can be
//! done.
//!
//! wlr-randr, kanshi and wdisplays describe a desk as a list of heads, each
//! with the modes it offers, and ask for a new one all at once: this one at
//! 2560x1440 and scale 1.25 there, that one off. This module is the arithmetic
//! of whether such a request makes sense, before any monitor is touched: the
//! compositor carries out only a request [`check_heads`] accepted, so a typo in a
//! kanshi profile is refused whole rather than half applied.

use crate::geometry::FAR;

/// One monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    /// The connector name: `DP-1`, `HEADLESS-1`.
    pub name: String,
    pub enabled: bool,
    pub modes: Vec<HeadMode>,
    /// Which of `modes` it is showing, when enabled.
    pub current: Option<usize>,
    /// Its top-left corner on the desk, in logical pixels.
    pub position: (i32, i32),
    pub scale: f64,
}

/// A mode a monitor offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadMode {
    pub width: i32,
    pub height: i32,
    /// In millihertz, as the protocol counts it.
    pub refresh: i32,
    pub preferred: bool,
}

/// Which mode a request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChoice {
    /// One of the head's own, by index.
    Listed(usize),
    /// A size and refresh the head did not offer.
    Custom {
        width: i32,
        height: i32,
        refresh: i32,
    },
}

/// What a request says about one head. Every head is in a request, enabled
/// or not; what is left `None` stays as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadChange {
    pub name: String,
    pub enabled: bool,
    pub mode: Option<ModeChoice>,
    pub position: Option<(i32, i32)>,
    pub scale: Option<f64>,
}

/// Why a request was refused. Nothing of it is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// It would leave no monitor on.
    NoOutputLeft,
    /// It names a monitor there is not.
    UnknownHead(String),
    /// It asks a monitor for a mode it does not have.
    UnknownMode(String),
    /// It asks for a mode of its own making, which this monitor cannot show.
    CustomModeUnsupported(String),
    /// A custom mode of no size, or one wider or taller than the world.
    BadMode(String),
    /// A scale outside 0.5 to 4.
    BadScale(String),
    /// A position further than the world's edge from the first monitor's
    /// corner, either way: past where a sum of it and a width is an `i32`
    /// (#154).
    BadPosition(String),
}

/// The smallest and largest scale a monitor may be given: a display tool's
/// request, and a config file's `[[output]] scale`.
pub const SCALES: std::ops::RangeInclusive<f64> = 0.5..=4.0;

/// Which of `offered` a monitor shows when asked for `wanted` (a width,
/// height and refresh in Hz, as a config file writes a mode), by index, and
/// whether that is what was asked.
///
/// With no request, the monitor's preferred mode (or its first, for one that
/// prefers none). With a request, the offered mode of that size whose refresh
/// is nearest the one asked for, or the fastest at that size if no refresh was
/// given: someone who writes `2560x1440` for a 144 Hz monitor means 144 Hz.
/// A size the monitor does not offer falls back to preferred, and the caller
/// says so, so a typo cannot leave a screen dark.
#[must_use]
pub fn pick_mode(
    wanted: Option<(u16, u16, Option<f64>)>,
    offered: &[HeadMode],
) -> Option<(usize, bool)> {
    let preferred = || {
        offered
            .iter()
            .position(|mode| mode.preferred)
            .or((!offered.is_empty()).then_some(0))
    };
    let Some((width, height, refresh)) = wanted else {
        return preferred().map(|at| (at, true));
    };
    let sized = offered
        .iter()
        .enumerate()
        .filter(|(_, mode)| mode.width == i32::from(width) && mode.height == i32::from(height));
    let chosen = match refresh {
        Some(hz) => {
            let off = |mode: &HeadMode| (mode.hertz() - hz).abs();
            sized.min_by(|(_, a), (_, b)| off(a).total_cmp(&off(b)))
        }
        None => sized.max_by_key(|(_, mode)| mode.refresh),
    };
    match chosen {
        Some((at, _)) => Some((at, true)),
        None => preferred().map(|at| (at, false)),
    }
}

impl HeadMode {
    /// Its refresh in Hz, as a config file writes it.
    #[must_use]
    pub fn hertz(&self) -> f64 {
        f64::from(self.refresh) / 1000.0
    }
}

/// Whether `changes` can be carried out on `heads`. `custom_modes` is whether
/// a monitor may be given a mode it does not list: true of a virtual one, not
/// of a real one, which would show nothing.
///
/// # Errors
///
/// The first thing wrong with it.
pub fn check_heads(
    heads: &[Head],
    changes: &[HeadChange],
    custom_modes: bool,
) -> Result<(), Rejection> {
    for change in changes {
        let Some(head) = heads.iter().find(|head| head.name == change.name) else {
            return Err(Rejection::UnknownHead(change.name.clone()));
        };
        match change.mode {
            Some(ModeChoice::Listed(at)) if at >= head.modes.len() => {
                return Err(Rejection::UnknownMode(head.name.clone()));
            }
            Some(ModeChoice::Custom { .. }) if !custom_modes => {
                return Err(Rejection::CustomModeUnsupported(head.name.clone()));
            }
            Some(ModeChoice::Custom { width, height, .. })
                if !(1..=FAR).contains(&width) || !(1..=FAR).contains(&height) =>
            {
                return Err(Rejection::BadMode(head.name.clone()));
            }
            _ => {}
        }
        if change.scale.is_some_and(|scale| !SCALES.contains(&scale)) {
            return Err(Rejection::BadScale(head.name.clone()));
        }
        let far = |at: i32| !(-FAR..=FAR).contains(&at);
        if change.position.is_some_and(|(x, y)| far(x) || far(y)) {
            return Err(Rejection::BadPosition(head.name.clone()));
        }
    }
    let on = heads.iter().any(|head| {
        changes
            .iter()
            .find(|change| change.name == head.name)
            .map_or(head.enabled, |change| change.enabled)
    });
    if on {
        Ok(())
    } else {
        Err(Rejection::NoOutputLeft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(name: &str, enabled: bool) -> Head {
        Head {
            name: name.to_owned(),
            enabled,
            modes: vec![HeadMode {
                width: 1920,
                height: 1080,
                refresh: 60_000,
                preferred: true,
            }],
            current: enabled.then_some(0),
            position: (0, 0),
            scale: 1.0,
        }
    }

    fn change(name: &str, enabled: bool) -> HeadChange {
        HeadChange {
            name: name.to_owned(),
            enabled,
            mode: None,
            position: None,
            scale: None,
        }
    }

    #[test]
    fn a_request_naming_every_head_sensibly_is_accepted() {
        let heads = [head("DP-1", true), head("HDMI-A-1", false)];
        let changes = [
            HeadChange {
                position: Some((1920, 0)),
                scale: Some(1.25),
                mode: Some(ModeChoice::Listed(0)),
                ..change("DP-1", true)
            },
            change("HDMI-A-1", true),
        ];
        assert_eq!(check_heads(&heads, &changes, false), Ok(()));
    }

    #[test]
    fn turning_every_monitor_off_is_refused() {
        let heads = [head("DP-1", true), head("HDMI-A-1", false)];
        assert_eq!(
            check_heads(
                &heads,
                &[change("DP-1", false), change("HDMI-A-1", false)],
                false
            ),
            Err(Rejection::NoOutputLeft)
        );
    }

    #[test]
    fn a_monitor_or_mode_there_is_not_is_refused_by_name() {
        let heads = [head("DP-1", true)];
        assert_eq!(
            check_heads(&heads, &[change("DP-9", true)], false),
            Err(Rejection::UnknownHead("DP-9".to_owned()))
        );
        let listed = HeadChange {
            mode: Some(ModeChoice::Listed(3)),
            ..change("DP-1", true)
        };
        assert_eq!(
            check_heads(&heads, &[listed], false),
            Err(Rejection::UnknownMode("DP-1".to_owned()))
        );
    }

    #[test]
    fn a_custom_mode_is_for_a_monitor_that_can_show_one() {
        let heads = [head("HEADLESS-1", true)];
        let custom = HeadChange {
            mode: Some(ModeChoice::Custom {
                width: 1280,
                height: 720,
                refresh: 60_000,
            }),
            ..change("HEADLESS-1", true)
        };
        assert_eq!(
            check_heads(&heads, std::slice::from_ref(&custom), false),
            Err(Rejection::CustomModeUnsupported("HEADLESS-1".to_owned()))
        );
        assert_eq!(check_heads(&heads, &[custom], true), Ok(()));
    }

    #[test]
    fn a_custom_mode_wider_than_the_world_is_refused() {
        let heads = [head("HEADLESS-1", true)];
        for (width, height) in [(FAR + 1, 720), (1280, i32::MAX)] {
            let custom = HeadChange {
                mode: Some(ModeChoice::Custom {
                    width,
                    height,
                    refresh: 60_000,
                }),
                ..change("HEADLESS-1", true)
            };
            assert_eq!(
                check_heads(&heads, &[custom], true),
                Err(Rejection::BadMode("HEADLESS-1".to_owned())),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn a_monitor_placed_past_the_world_is_refused() {
        let heads = [head("DP-1", true), head("HDMI-A-1", true)];
        for position in [(i32::MAX, 0), (0, i32::MIN), (FAR + 1, 0)] {
            let placed = HeadChange {
                position: Some(position),
                ..change("HDMI-A-1", true)
            };
            assert_eq!(
                check_heads(&heads, &[placed], false),
                Err(Rejection::BadPosition("HDMI-A-1".to_owned())),
                "{position:?}"
            );
        }
        let at_the_edge = HeadChange {
            position: Some((-FAR, FAR)),
            ..change("HDMI-A-1", true)
        };
        assert_eq!(check_heads(&heads, &[at_the_edge], false), Ok(()));
    }

    #[test]
    fn a_scale_outside_what_a_config_allows_is_refused() {
        let heads = [head("DP-1", true)];
        let scaled = HeadChange {
            scale: Some(8.0),
            ..change("DP-1", true)
        };
        assert_eq!(
            check_heads(&heads, &[scaled], false),
            Err(Rejection::BadScale("DP-1".to_owned()))
        );
    }

    fn mode(width: i32, height: i32, refresh: i32, preferred: bool) -> HeadMode {
        HeadMode {
            width,
            height,
            refresh,
            preferred,
        }
    }

    fn monitor() -> [HeadMode; 4] {
        [
            mode(1920, 1080, 60_000, false),
            mode(2560, 1440, 59_950, true),
            mode(2560, 1440, 143_910, false),
            mode(2560, 1440, 120_000, false),
        ]
    }

    #[test]
    fn with_no_request_the_monitors_preferred_mode_is_used() {
        assert_eq!(pick_mode(None, &monitor()), Some((1, true)));
    }

    #[test]
    fn a_size_alone_takes_the_fastest_refresh_at_that_size() {
        assert_eq!(
            pick_mode(Some((2560, 1440, None)), &monitor()),
            Some((2, true))
        );
    }

    #[test]
    fn a_refresh_takes_the_nearest_offered() {
        assert_eq!(
            pick_mode(Some((2560, 1440, Some(120.0))), &monitor()),
            Some((3, true))
        );
        assert_eq!(
            pick_mode(Some((2560, 1440, Some(144.0))), &monitor()),
            Some((2, true))
        );
    }

    #[test]
    fn a_refresh_is_matched_in_hertz_not_millihertz() {
        // 59.95 Hz is nearer 60 than 120 is; counted in millihertz against
        // a refresh written in Hz, every mode would be thousands off, and
        // the slowest would win.
        assert_eq!(
            pick_mode(Some((2560, 1440, Some(100.0))), &monitor()),
            Some((3, true))
        );
        assert_eq!(
            pick_mode(Some((2560, 1440, Some(60.0))), &monitor()),
            Some((1, true))
        );
    }

    #[test]
    fn a_size_the_monitor_lacks_falls_back_to_preferred_and_says_so() {
        assert_eq!(
            pick_mode(Some((800, 600, None)), &monitor()),
            Some((1, false))
        );
    }

    #[test]
    fn a_monitor_with_no_modes_has_nothing_to_pick() {
        assert_eq!(pick_mode(None, &[]), None);
    }
}
