//! Monitors as a display tool sees them, and whether what it asks for can be
//! done.
//!
//! wlr-randr, kanshi and wdisplays describe a desk as a list of heads, each
//! with the modes it offers, and ask for a new one all at once: this one at
//! 2560x1440 and scale 1.25 there, that one off. This module is the arithmetic
//! of whether such a request makes sense, before any monitor is touched: the
//! compositor carries out only a request [`check_heads`] accepted, so a typo in a
//! kanshi profile is refused whole rather than half applied.

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
    /// A custom mode of no size.
    BadMode(String),
    /// A scale outside 0.5 to 4.
    BadScale(String),
}

/// The smallest and largest scale a monitor may be given, as a config file's
/// `[[output]] scale` allows.
const SCALES: std::ops::RangeInclusive<f64> = 0.5..=4.0;

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
            Some(ModeChoice::Custom { width, height, .. }) if width <= 0 || height <= 0 => {
                return Err(Rejection::BadMode(head.name.clone()));
            }
            _ => {}
        }
        if change.scale.is_some_and(|scale| !SCALES.contains(&scale)) {
            return Err(Rejection::BadScale(head.name.clone()));
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
}
