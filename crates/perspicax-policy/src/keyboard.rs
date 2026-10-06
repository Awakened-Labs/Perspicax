//! Whose keyboard layout is in use: the session's, or each window's own.
//!
//! With [`Switching::Window`], a window is typed in the layout it was last
//! typed in, and a new window starts in the first. With
//! [`Switching::Global`], the default, the layout is the session's, and
//! switching it switches it everywhere.
//!
//! Only windows keep a layout. A panel, a menu or the lock screen taking the
//! keyboard changes nothing, and the window the keyboard comes back to is
//! typed in its own layout again, whatever was used in between.

/// Whose the layout in use is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Switching {
    /// The session's: switching it switches it everywhere.
    #[default]
    Global,
    /// Each window's own.
    Window,
}

/// The layout each window was last typed in, counted from 0.
#[derive(Debug, Clone)]
pub struct LayoutMemory<W> {
    switching: Switching,
    /// The window that has the keyboard, if a window has it.
    focused: Option<W>,
    layouts: Vec<(W, u32)>,
}

impl<W> Default for LayoutMemory<W> {
    fn default() -> Self {
        Self {
            switching: Switching::default(),
            focused: None,
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

    /// Change whose the layout is. A change forgets every window's layout:
    /// under `Global` there is none to keep, and one kept from before would
    /// be stale by the time `Window` is back.
    pub fn set_switching(&mut self, switching: Switching) {
        if switching != self.switching {
            *self = Self {
                switching,
                ..Self::default()
            };
        }
    }

    /// The keyboard moved to `window`, or to something that is not a window
    /// (`None`), while `active` was the layout in use. Remembers it as the
    /// layout of the window the keyboard left, and answers the layout to
    /// switch to, if it should change.
    pub fn focus(&mut self, window: Option<W>, active: u32) -> Option<u32> {
        if self.switching == Switching::Global {
            return None;
        }
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

    /// A window closed: nothing of it is kept.
    pub fn forget(&mut self, window: W) {
        self.layouts.retain(|(known, _)| *known != window);
        if self.focused == Some(window) {
            self.focused = None;
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

    #[test]
    fn under_global_switching_the_layout_never_follows_a_window() {
        let mut memory = LayoutMemory::default();
        assert_eq!(memory.focus(Some(1), 0), None);
        assert_eq!(memory.focus(Some(2), 1), None);
        assert_eq!(memory.focus(Some(1), 1), None);
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

    #[test]
    fn a_panel_or_menu_taking_the_keyboard_changes_nothing() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        // Window 1 in the second layout; a menu takes the keyboard, and is
        // typed in the first.
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
    fn changing_whose_the_layout_is_forgets_every_window() {
        let mut memory = per_window();
        memory.focus(Some(1), 0);
        memory.focus(Some(2), 1);
        memory.set_switching(Switching::Global);
        memory.set_switching(Switching::Window);
        assert_eq!(memory.focus(Some(1), 0), None, "nothing kept for 1");
    }
}
