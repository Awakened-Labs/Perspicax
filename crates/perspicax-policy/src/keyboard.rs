//! Whose keyboard layout is in use: the session's, or each window's own.
//!
//! With [`Switching::Window`], a window is typed in the layout it was last
//! typed in, and a new window starts in the first. With
//! [`Switching::Global`], the default, the layout is the session's, and
//! switching it switches it everywhere.
//!
//! Only windows keep a layout. A panel, the start menu or the lock screen
//! taking the keyboard changes nothing, and the window the keyboard comes
//! back to is typed in its own layout again, whatever was used in between. A
//! window's own menu is the window's: the compositor names the window, not
//! `None`, while the menu has the keyboard.
//!
//! The keyboard's moving and the layout's following it are two steps. The
//! compositor is told of a move where it cannot read or switch the layout,
//! so it notes it ([`LayoutMemory::moved`]) and follows it
//! ([`LayoutMemory::follow`]) before the layout is next read, switched or
//! typed in. A switch made after the keyboard moved is then the new
//! window's, and is not undone (#237).

/// Whose the layout in use is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Switching {
    /// The session's: switching it switches it everywhere.
    #[default]
    Global,
    /// Each window's own.
    Window,
}

impl Switching {
    pub const ALL: [Self; 2] = [Self::Global, Self::Window];

    /// Its name as `[input.keyboard] switching` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Window => "window",
        }
    }
}

/// The layout each window was last typed in, counted from 0.
#[derive(Debug, Clone)]
pub struct LayoutMemory<W> {
    switching: Switching,
    /// The window the layout last followed the keyboard to, if a window has
    /// it.
    focused: Option<W>,
    /// Where the keyboard has gone since, if it has moved: `Some(None)` for
    /// something that is not a window.
    moved: Option<Option<W>>,
    layouts: Vec<(W, u32)>,
}

impl<W> Default for LayoutMemory<W> {
    fn default() -> Self {
        Self {
            switching: Switching::default(),
            focused: None,
            moved: None,
            layouts: Vec::new(),
        }
    }
}

impl<W: Copy + PartialEq> LayoutMemory<W> {
    /// Whose the layout is now.
    #[must_use]
    pub fn switching(&self) -> Switching {
        self.switching
    }

    /// Change whose the layout is. A change forgets every window's layout,
    /// and a move not yet followed: under `Global` there is none to keep,
    /// and one kept from before would be stale by the time `Window` is back.
    pub fn set_switching(&mut self, switching: Switching) {
        if switching != self.switching {
            *self = Self {
                switching,
                ..Self::default()
            };
        }
    }

    /// The keyboard moved to `window`, or to something that is not a window
    /// (`None`). Only noted: the layout follows at [`Self::follow`]. Moved
    /// again first, it follows to the last; a window passed through was
    /// typed in no layout, and keeps the one it had.
    pub fn moved(&mut self, window: Option<W>) {
        if self.switching == Switching::Window {
            self.moved = Some(window);
        }
    }

    /// Whether the keyboard has moved since the layout last followed it.
    #[must_use]
    pub fn has_moved(&self) -> bool {
        self.moved.is_some()
    }

    /// Follow the keyboard where it moved, while `active` is the layout in
    /// use: remember it as the layout of the window the keyboard left, and
    /// answer the layout to switch to, if it should change. Nothing, if it
    /// has not moved.
    pub fn follow(&mut self, active: u32) -> Option<u32> {
        let window = self.moved.take()?;
        if let Some(left) = self.focused.take() {
            match self.layouts.iter_mut().find(|(known, _)| *known == left) {
                Some((_, layout)) => *layout = active,
                None => self.layouts.push((left, active)),
            }
        }
        let window = window?;
        self.focused = Some(window);
        let layout = self
            .layouts
            .iter()
            .find(|(known, _)| *known == window)
            .map_or(0, |&(_, layout)| layout);
        (layout != active).then_some(layout)
    }

    /// A window closed: nothing of it is kept. A move to it not yet followed
    /// still leaves the window the keyboard was in, but goes nowhere.
    pub fn forget(&mut self, window: W) {
        self.layouts.retain(|(known, _)| *known != window);
        if self.focused == Some(window) {
            self.focused = None;
        }
        if self.moved == Some(Some(window)) {
            self.moved = Some(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn per_window() -> LayoutMemory<u8> {
        let mut memory = LayoutMemory::default();
        memory.set_switching(Switching::Window);
        memory
    }

    /// The keyboard moving and the layout following at once, as each test
    /// before #237 had it.
    trait Focus {
        /// The keyboard moved to `window`, and the layout followed while
        /// `active` was in use.
        fn focus(&mut self, window: Option<u8>, active: u32) -> Option<u32>;
    }

    impl Focus for LayoutMemory<u8> {
        fn focus(&mut self, window: Option<u8>, active: u32) -> Option<u32> {
            self.moved(window);
            self.follow(active)
        }
    }

    #[test]
    fn under_global_switching_the_layout_never_follows_a_window() {
        let mut memory = LayoutMemory::default();
        assert_eq!(memory.focus(Some(1), 0), None);
        assert_eq!(memory.focus(Some(2), 1), None);
        assert_eq!(memory.focus(Some(1), 1), None);
        memory.moved(Some(2));
        assert!(!memory.has_moved(), "nothing to follow");
    }

    #[test]
    fn each_window_gets_back_the_layout_it_was_typed_in() {
        let mut memory = per_window();
        assert_eq!(memory.focus(Some(1), 0), None, "a first window: the first");
        // Switched to the second layout in window 1, then window 2 opens.
        assert_eq!(memory.focus(Some(2), 1), Some(0), "a new window: the first");
        assert_eq!(memory.focus(Some(1), 0), Some(1), "window 1 had the second");
        assert_eq!(memory.focus(Some(2), 1), Some(0), "and window 2 the first");
    }

    /// #237: what the window left was typed in is the layout in use when
    /// the layout follows, not when the keyboard moved. So the compositor
    /// follows before any switch, and a switch made after is the new
    /// window's.
    #[test]
    fn a_move_is_followed_once_with_the_layout_in_use_then() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        memory.moved(Some(2));
        assert!(memory.has_moved());
        // Followed with the second layout in use: window 1 was left in it.
        assert_eq!(memory.follow(1), Some(0), "2 is new: the first");
        assert!(!memory.has_moved(), "followed");
        assert_eq!(memory.follow(0), None, "and only once");
        assert_eq!(memory.focus(Some(1), 0), Some(1), "1 kept the second");
    }

    #[test]
    fn moved_twice_before_following_the_layout_follows_to_the_last() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        memory.focus(Some(2), 0);
        // Window 2 in the second layout; the keyboard passes through 1 to 3.
        memory.moved(Some(1));
        memory.moved(Some(3));
        assert_eq!(memory.follow(1), Some(0), "3 is new: the first");
        assert_eq!(
            memory.focus(Some(2), 0),
            Some(1),
            "2 was left in the second"
        );
        assert_eq!(memory.focus(Some(1), 1), Some(0), "1 kept its first");
    }

    #[test]
    fn a_panel_or_menu_taking_the_keyboard_changes_nothing() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        // Window 1 in the second layout; the start menu takes the keyboard,
        // and is typed in the first.
        assert_eq!(memory.focus(None, 1), None);
        assert_eq!(
            memory.focus(Some(1), 0),
            Some(1),
            "back to window 1: its own layout, not the menu's"
        );
    }

    #[test]
    fn a_closed_window_is_forgotten() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        memory.focus(Some(2), 1);
        memory.forget(1);
        assert_eq!(
            memory.focus(Some(1), 0),
            None,
            "a new 1 starts in the first, not the second the old one had"
        );
        // Closed while it has the keyboard: leaving it records nothing.
        memory.forget(1);
        memory.focus(None, 1);
        assert_eq!(memory.focus(Some(1), 1), Some(0), "1 left nothing behind");
    }

    #[test]
    fn a_window_closed_before_the_layout_followed_it_there_is_never_switched_to() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        memory.moved(Some(2));
        memory.forget(2);
        assert!(memory.has_moved(), "1 was still left");
        assert_eq!(memory.follow(1), None, "nothing to switch to");
        assert_eq!(
            memory.focus(Some(1), 0),
            Some(1),
            "1 kept the second it was left in"
        );
    }

    #[test]
    fn changing_whose_the_layout_is_forgets_every_window() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        memory.focus(Some(2), 1);
        memory.moved(Some(1));
        memory.set_switching(Switching::Global);
        memory.set_switching(Switching::Window);
        assert!(!memory.has_moved(), "nor a move not yet followed");
        assert_eq!(memory.focus(Some(1), 0), None, "nothing kept for 1");
    }
}
