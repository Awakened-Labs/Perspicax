//! The keyboard's layouts, as perspicax lists them on the shell channel:
//! each one's name and short label, and which is in use. What the panel's
//! layout indicator shows, and where a click on it moves to.

/// One of the keyboard's layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    /// xkb's name for it, as "English (US)".
    pub(crate) name: String,
    /// A label for a panel, as "US".
    pub(crate) short: String,
}

/// The keyboard's layouts and the one in use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Layouts {
    listed: Vec<Layout>,
    /// Named since the last list was done.
    listing: Vec<Layout>,
    active: u32,
}

impl Layouts {
    /// The next layout in the list being sent.
    pub(crate) fn layout(&mut self, name: String, short: String) {
        self.listing.push(Layout { name, short });
    }

    /// The list being sent is complete, and replaces the one before.
    pub(crate) fn done(&mut self) {
        self.listed = std::mem::take(&mut self.listing);
    }

    /// The layout in use is now this one, counted from 0.
    pub(crate) fn activate(&mut self, index: u32) {
        self.active = index;
    }

    /// The layout in use, when there is a choice of layouts to show: none
    /// while the keyboard has only one, or perspicax has named none.
    pub(crate) fn shown(&self) -> Option<&Layout> {
        if self.listed.len() < 2 {
            return None;
        }
        self.listed.get(usize::try_from(self.active).ok()?)
    }

    /// The layout after the one in use, wrapping to the first: where a
    /// click on the indicator moves to.
    pub(crate) fn next(&self) -> Option<u32> {
        self.shown()?;
        let count = u32::try_from(self.listed.len()).ok()?;
        Some((self.active + 1) % count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us_ru() -> Layouts {
        let mut layouts = Layouts::default();
        layouts.layout("English (US)".to_owned(), "US".to_owned());
        layouts.layout("Russian".to_owned(), "RU".to_owned());
        layouts.done();
        layouts
    }

    #[test]
    fn the_layout_in_use_is_shown_and_a_click_moves_to_the_next() {
        let mut layouts = us_ru();
        assert_eq!(
            layouts.shown().map(|layout| layout.short.as_str()),
            Some("US")
        );
        assert_eq!(layouts.next(), Some(1));
        layouts.activate(1);
        assert_eq!(
            layouts.shown().map(|layout| layout.name.as_str()),
            Some("Russian")
        );
        assert_eq!(layouts.next(), Some(0), "round to the first");
    }

    #[test]
    fn one_layout_is_no_choice_and_nothing_is_shown() {
        let mut layouts = Layouts::default();
        assert_eq!(layouts.shown(), None, "none named yet");
        layouts.layout("English (US)".to_owned(), "US".to_owned());
        layouts.done();
        assert_eq!(layouts.shown(), None);
        assert_eq!(layouts.next(), None);
    }

    #[test]
    fn a_list_is_used_once_done_and_replaces_the_one_before() {
        let mut layouts = us_ru();
        layouts.layout("German".to_owned(), "DE".to_owned());
        assert_eq!(
            layouts.shown().map(|layout| layout.short.as_str()),
            Some("US"),
            "not done yet"
        );
        layouts.done();
        assert_eq!(layouts.shown(), None, "one layout now");
    }
}
