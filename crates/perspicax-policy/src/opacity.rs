//! Opacity: how far the person has made a window see-through, for their own
//! eyes.
//!
//! A window starts at its application's rule, or opaque. The person's keys
//! move it from there a step at a time, never below a floor that keeps it from
//! vanishing, and what the keys set stays with the window. A window without
//! the keyboard is drawn dimmer again, by a share of whatever it has. Every
//! value is a whole percent, from 1 to 100.
//!
//! None of this reaches an agent. What a window covers is judged from what its
//! client declared and from its frame (`perspicax-index`), and a window the
//! person made half see-through still covers what is behind it: the person
//! chose to look through it, and an agent is not expected to read what shows.
//! What the compositor draws at full opacity whatever is asked, a fullscreen
//! window for one, is the compositor's to decide.

use std::collections::BTreeMap;

/// Wholly opaque, in percent.
pub const OPAQUE: u8 = 100;

/// The `[opacity]` table: how the keys move a window's opacity, how far a
/// window without the keyboard is dimmed, and where each application's
/// windows start. Every value a whole percent, from 1 to 100.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opacity {
    /// How far one press of `opacity-up` or `opacity-down` moves a window.
    pub step: u8,
    /// The lowest the keys take a window, so that it never vanishes.
    pub floor: u8,
    /// What a window without the keyboard is drawn at, as a share of its
    /// own. 100 dims nothing.
    pub unfocused: u8,
    /// Where each application's windows start, by Wayland app id or X11
    /// `WM_CLASS` class, matched exactly.
    pub apps: BTreeMap<String, u8>,
}

impl Default for Opacity {
    /// Nothing see-through: no application's rule, and no dimming.
    fn default() -> Self {
        Self {
            step: 10,
            floor: 20,
            unfocused: OPAQUE,
            apps: BTreeMap::new(),
        }
    }
}

impl Opacity {
    /// Where a window of `app` starts: its application's rule, or opaque.
    #[must_use]
    pub fn ruled(&self, app: Option<&str>) -> u8 {
        app.and_then(|app| self.apps.get(app))
            .copied()
            .unwrap_or(OPAQUE)
    }

    /// One press of a key from `now`: a step up, or a step down, within the
    /// floor and opaque.
    ///
    /// Never the wrong way. A window the keys left below a floor that a
    /// reload has since raised stays where it is going down, and rises going
    /// up.
    #[must_use]
    pub fn stepped(&self, now: u8, up: bool) -> u8 {
        if up {
            now.saturating_add(self.step).min(OPAQUE).max(now)
        } else if now <= self.floor {
            now
        } else {
            now.saturating_sub(self.step).max(self.floor)
        }
    }

    /// What a window whose own opacity is `own` is drawn at: its own while it
    /// is in use, and `unfocused` of it while it is not, to the nearest
    /// percent. Dimming multiplies, so it can go below the floor, which is
    /// only how far the keys go.
    #[must_use]
    pub fn shown(&self, own: u8, in_use: bool) -> u8 {
        if in_use {
            return own;
        }
        let dimmed = (u16::from(own) * u16::from(self.unfocused) + 50) / 100;
        u8::try_from(dimmed).unwrap_or(OPAQUE).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_apps(apps: &[(&str, u8)]) -> Opacity {
        Opacity {
            apps: apps
                .iter()
                .map(|&(app, percent)| (app.to_owned(), percent))
                .collect(),
            ..Opacity::default()
        }
    }

    #[test]
    fn by_default_nothing_is_see_through() {
        let opacity = Opacity::default();
        assert_eq!(opacity.ruled(Some("foot")), OPAQUE);
        assert_eq!(opacity.ruled(None), OPAQUE);
        assert_eq!(opacity.shown(OPAQUE, false), OPAQUE);
        assert_eq!(opacity.shown(OPAQUE, true), OPAQUE);
    }

    #[test]
    fn a_window_starts_at_its_applications_rule_matched_exactly() {
        let opacity = with_apps(&[("foot", 90), ("org.gnome.Nautilus", 95)]);
        assert_eq!(opacity.ruled(Some("foot")), 90);
        assert_eq!(opacity.ruled(Some("org.gnome.Nautilus")), 95);
        assert_eq!(opacity.ruled(Some("Foot")), OPAQUE, "case matters");
        assert_eq!(opacity.ruled(Some("footclient")), OPAQUE);
        assert_eq!(opacity.ruled(None), OPAQUE, "a window with no app id");
    }

    #[test]
    fn the_keys_step_within_the_floor_and_opaque() {
        let opacity = Opacity::default();
        assert_eq!(opacity.stepped(OPAQUE, false), 90);
        assert_eq!(opacity.stepped(90, true), OPAQUE);
        assert_eq!(opacity.stepped(OPAQUE, true), OPAQUE, "opaque is the top");
        assert_eq!(opacity.stepped(95, true), OPAQUE, "a rule off the step");
        assert_eq!(opacity.stepped(25, false), 20, "stops at the floor");
        assert_eq!(opacity.stepped(20, false), 20);
    }

    #[test]
    fn below_a_raised_floor_the_keys_never_go_the_wrong_way() {
        let opacity = Opacity {
            floor: 40,
            ..Opacity::default()
        };
        assert_eq!(opacity.stepped(30, false), 30, "down stays put");
        assert_eq!(opacity.stepped(30, true), 40, "up still rises");
        assert_eq!(opacity.stepped(45, false), 40);
    }

    #[test]
    fn a_window_without_the_keyboard_is_dimmed_by_a_share_of_its_own() {
        let opacity = Opacity {
            unfocused: 85,
            ..Opacity::default()
        };
        assert_eq!(opacity.shown(90, true), 90, "in use, its own");
        assert_eq!(opacity.shown(90, false), 77, "90 × 85%, rounded");
        assert_eq!(opacity.shown(OPAQUE, false), 85);
    }

    #[test]
    fn dimming_can_go_below_the_floor_but_never_to_nothing() {
        let opacity = Opacity {
            unfocused: 20,
            ..Opacity::default()
        };
        assert_eq!(opacity.shown(20, false), 4);
        let faintest = Opacity {
            unfocused: 1,
            floor: 1,
            ..Opacity::default()
        };
        assert_eq!(faintest.shown(1, false), 1);
    }
}
