//! Where each program's window was when it last closed, so that its next one
//! can open there.
//!
//! A Wayland window cannot ask where to open: xdg-shell has no request for a
//! position, so a program that saves its own place has nowhere to put it.
//! The compositor remembers instead, one place per program, by its Wayland
//! app id or its X11 `WM_CLASS` class: the monitor its window was on, by the
//! connector's name, and where its corner was on that monitor. Not its size,
//! which a program chooses for itself as it first draws, and never its title,
//! which carries document names, URLs and chat subjects.
//!
//! The places are the person's history, not the state of any window open
//! now. Only the programs closed most recently are kept, and none of it
//! reaches an agent.

use std::collections::BTreeSet;

/// How many programs' places are kept: those whose windows closed most
/// recently.
pub const REMEMBERED: usize = 200;

/// The longest app id remembered, in bytes: D-Bus's limit for a name, which a
/// reverse-DNS app id is. Any longer, and one program could fill the file the
/// places are kept in.
pub const APP_ID_MAX: usize = 255;

/// Where a program's window was: on the monitor named `output`, by its
/// connector, with its corner at `at`, in logical pixels from that monitor's
/// own corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    pub output: String,
    pub at: (i32, i32),
}

/// Each program's last place, the most recently closed first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    newest_first: Vec<(String, Spot)>,
}

impl Places {
    /// Places read back as [`Self::newest_first`] gave them. The first of
    /// each program is kept, up to [`REMEMBERED`] of them, and one no program
    /// could be named is left out.
    #[must_use]
    pub fn from_newest_first(places: impl IntoIterator<Item = (String, Spot)>) -> Self {
        let mut kept: Vec<(String, Spot)> = Vec::new();
        for (app, spot) in places {
            if kept.len() == REMEMBERED {
                break;
            }
            if nameable(&app) && !kept.iter().any(|(known, _)| *known == app) {
                kept.push((app, spot));
            }
        }
        Self { newest_first: kept }
    }

    /// A window of `app` closed at `spot`: that is its place now, and it is
    /// the newest. The oldest beyond [`REMEMBERED`] is forgotten. Whether
    /// anything changed.
    pub fn remember(&mut self, app: &str, spot: Spot) -> bool {
        if !nameable(app)
            || self
                .newest_first
                .first()
                .is_some_and(|(newest, at)| newest == app && *at == spot)
        {
            return false;
        }
        self.newest_first.retain(|(known, _)| known != app);
        self.newest_first.insert(0, (app.to_owned(), spot));
        self.newest_first.truncate(REMEMBERED);
        true
    }

    /// Where `app`'s window last closed.
    #[must_use]
    pub fn recall(&self, app: &str) -> Option<&Spot> {
        self.newest_first
            .iter()
            .find(|(known, _)| known == app)
            .map(|(_, spot)| spot)
    }

    /// Keep only the programs `keep` says to. Whether any went.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) -> bool {
        let before = self.newest_first.len();
        self.newest_first.retain(|(app, _)| keep(app));
        self.newest_first.len() != before
    }

    /// Every program's place, the most recently closed first.
    pub fn newest_first(&self) -> impl Iterator<Item = (&str, &Spot)> {
        self.newest_first
            .iter()
            .map(|(app, spot)| (app.as_str(), spot))
    }
}

/// The `[placement]` table: whether a program's place is remembered at all,
/// and the programs whose never is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembering {
    pub on: bool,
    /// Never remembered, by Wayland app id or X11 `WM_CLASS` class, matched
    /// exactly.
    pub forget: BTreeSet<String>,
}

impl Default for Remembering {
    /// Every program remembered.
    fn default() -> Self {
        Self {
            on: true,
            forget: BTreeSet::new(),
        }
    }
}

impl Remembering {
    /// Whether `app`'s place is remembered.
    #[must_use]
    pub fn remembers(&self, app: &str) -> bool {
        self.on && nameable(app) && !self.forget.contains(app)
    }
}

/// Whether a program could be named `app`: not by nothing, and not by more
/// than [`APP_ID_MAX`] bytes.
fn nameable(app: &str) -> bool {
    !app.is_empty() && app.len() <= APP_ID_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(output: &str, x: i32, y: i32) -> Spot {
        Spot {
            output: output.to_owned(),
            at: (x, y),
        }
    }

    fn apps(places: &Places) -> Vec<&str> {
        places.newest_first().map(|(app, _)| app).collect()
    }

    #[test]
    fn a_program_is_recalled_where_its_window_last_closed() {
        let mut places = Places::default();
        assert!(places.remember("ghostty", on("DP-1", 960, 0)));
        assert_eq!(places.recall("ghostty"), Some(&on("DP-1", 960, 0)));
        assert_eq!(places.recall("firefox"), None, "never closed");
        assert!(places.remember("ghostty", on("DP-1", 0, 540)));
        assert_eq!(
            places.recall("ghostty"),
            Some(&on("DP-1", 0, 540)),
            "closed again somewhere else, with nothing closed in between"
        );
    }

    #[test]
    fn a_program_closed_again_is_recalled_at_its_new_place_and_is_the_newest() {
        let mut places = Places::default();
        places.remember("ghostty", on("DP-1", 960, 0));
        places.remember("firefox", on("DP-1", 0, 0));
        assert!(places.remember("ghostty", on("HDMI-A-1", 100, 50)));
        assert_eq!(places.recall("ghostty"), Some(&on("HDMI-A-1", 100, 50)));
        assert_eq!(apps(&places), ["ghostty", "firefox"], "once each");
    }

    #[test]
    fn the_newest_program_closed_at_the_same_place_again_changes_nothing() {
        let mut places = Places::default();
        places.remember("firefox", on("DP-1", 0, 0));
        places.remember("ghostty", on("DP-1", 960, 0));
        assert!(!places.remember("ghostty", on("DP-1", 960, 0)));
        assert!(
            places.remember("firefox", on("DP-1", 0, 0)),
            "the same place, but now the newest, so the last to be forgotten"
        );
        assert_eq!(apps(&places), ["firefox", "ghostty"]);
    }

    #[test]
    fn past_the_cap_the_program_closed_longest_ago_is_forgotten() {
        let mut places = Places::default();
        for nth in 0..REMEMBERED {
            places.remember(&format!("app{nth}"), on("DP-1", 0, 0));
        }
        // The oldest closed again, so the second oldest is the one to go.
        places.remember("app0", on("DP-1", 10, 10));
        places.remember("one-more", on("DP-1", 0, 0));
        assert_eq!(places.newest_first().count(), REMEMBERED);
        assert_eq!(places.recall("app1"), None);
        assert_eq!(places.recall("app0"), Some(&on("DP-1", 10, 10)));
        assert!(places.recall("one-more").is_some());
    }

    #[test]
    fn a_program_no_one_could_name_is_not_remembered() {
        let mut places = Places::default();
        assert!(!places.remember("", on("DP-1", 0, 0)));
        assert!(!places.remember(&"a".repeat(APP_ID_MAX + 1), on("DP-1", 0, 0)));
        assert!(places.remember(&"a".repeat(APP_ID_MAX), on("DP-1", 0, 0)));
        assert_eq!(places.newest_first().count(), 1);
    }

    #[test]
    fn programs_kept_out_are_recalled_nowhere() {
        let mut places = Places::default();
        places.remember("keepassxc", on("DP-1", 0, 0));
        places.remember("ghostty", on("DP-1", 960, 0));
        assert!(places.retain(|app| app != "keepassxc"));
        assert_eq!(places.recall("keepassxc"), None);
        assert_eq!(apps(&places), ["ghostty"]);
        assert!(!places.retain(|app| app != "keepassxc"), "already gone");
    }

    #[test]
    fn places_read_back_keep_the_first_of_each_program_and_no_more_than_the_cap() {
        let read = (0..REMEMBERED + 100)
            .map(|nth| (format!("app{nth}"), on("DP-1", 0, 0)))
            .chain([("app0".to_owned(), on("DP-2", 5, 5))]);
        let places = Places::from_newest_first(
            [
                ("app0".to_owned(), on("DP-1", 1, 1)),
                (String::new(), on("DP-1", 0, 0)),
            ]
            .into_iter()
            .chain(read),
        );
        assert_eq!(places.newest_first().count(), REMEMBERED);
        assert_eq!(places.recall("app0"), Some(&on("DP-1", 1, 1)), "the first");
        assert_eq!(places.recall(""), None);
        assert!(places.recall(&format!("app{}", REMEMBERED - 1)).is_some());
        assert_eq!(places.recall(&format!("app{REMEMBERED}")), None);
    }

    #[test]
    fn every_program_is_remembered_unless_the_person_says_not_or_lists_it_to_forget() {
        let mut remembering = Remembering::default();
        assert!(remembering.remembers("ghostty"));
        assert!(!remembering.remembers(""), "nothing to know it by");
        remembering
            .forget
            .insert("org.keepassxc.KeePassXC".to_owned());
        assert!(!remembering.remembers("org.keepassxc.KeePassXC"));
        assert!(
            remembering.remembers("org.keepassxc.keepassxc"),
            "matched exactly"
        );
        remembering.on = false;
        assert!(!remembering.remembers("ghostty"));
    }
}
