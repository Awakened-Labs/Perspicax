//! A tap of the Logo key: pressed and let go with nothing else in between.
//!
//! On Plasma and Windows a tap of the Logo key opens the start menu, and the
//! same key held down is the modifier of a dozen chords: Logo+Left snaps a
//! window, Logo+Tab steps through tabs. A tap can therefore only be known at
//! the release, once nothing else happened while the key was down. Any other
//! key pressed meanwhile makes it a chord, and a button or the wheel makes it
//! a mouse gesture, and neither is a tap.
//!
//! The compositor still forwards the press and the release to the focused
//! client, as it would any modifier. Swallowing them would leave the client
//! believing the key was never pressed, and every Logo chord the client binds
//! for itself would stop working.

use xkeysym::Keysym;

use crate::Mods;

/// Whether a keysym is one of the Logo keys. xkb calls both of them Super.
#[must_use]
pub fn is_logo(sym: Keysym) -> bool {
    sym == Keysym::Super_L || sym == Keysym::Super_R
}

/// Watches the keys and the pointer for a tap of the Logo key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogoTap {
    armed: bool,
}

impl LogoTap {
    /// A key went down. `logo` says whether it is a Logo key, and `mods` are
    /// the modifiers held with it, its own included. Only a Logo key pressed
    /// with no other modifier can begin a tap, and any other key ends one.
    pub fn press(&mut self, logo: bool, mods: Mods) {
        self.armed = logo
            && mods
                == Mods {
                    logo: true,
                    ..Mods::default()
                };
    }

    /// A button was pressed or the wheel turned while the key was down: a
    /// drag with Logo held, say. Not a tap.
    pub fn interrupt(&mut self) {
        self.armed = false;
    }

    /// A key came up. Whether that was the end of a tap.
    #[must_use]
    pub fn release(&mut self, logo: bool) -> bool {
        let tapped = self.armed && logo;
        self.armed = false;
        tapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logo() -> Mods {
        Mods {
            logo: true,
            ..Mods::default()
        }
    }

    #[test]
    fn a_logo_key_pressed_and_released_alone_is_a_tap() {
        let mut tap = LogoTap::default();
        tap.press(true, logo());
        assert!(tap.release(true));
    }

    #[test]
    fn a_key_pressed_while_logo_is_held_is_not_a_tap() {
        let mut tap = LogoTap::default();
        tap.press(true, logo());
        tap.press(false, logo());
        assert!(!tap.release(false));
        assert!(!tap.release(true));
    }

    #[test]
    fn a_click_or_scroll_while_logo_is_held_is_not_a_tap() {
        let mut tap = LogoTap::default();
        tap.press(true, logo());
        tap.interrupt();
        assert!(!tap.release(true));
    }

    #[test]
    fn logo_with_shift_held_is_not_a_tap() {
        let mut tap = LogoTap::default();
        tap.press(
            true,
            Mods {
                shift: true,
                ..logo()
            },
        );
        assert!(!tap.release(true));
    }

    #[test]
    fn a_key_held_from_before_and_let_go_is_not_a_tap() {
        let mut tap = LogoTap::default();
        tap.press(true, logo());
        assert!(!tap.release(false), "another key came up first");
        assert!(!tap.release(true));
    }

    #[test]
    fn only_the_two_super_keys_are_logo() {
        assert!(is_logo(Keysym::Super_L));
        assert!(is_logo(Keysym::Super_R));
        assert!(!is_logo(Keysym::Alt_L));
        assert!(!is_logo(Keysym::Menu));
    }
}
