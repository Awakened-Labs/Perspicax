//! The seat's keyboard as a person keeps it: a keymap that can change under
//! them without losing their place, and layouts to switch between.
//!
//! Three things on a keyboard are set by hand and expected to stay set: the
//! layout in use, Caps Lock and Num Lock. xkb keeps all three in its state,
//! and a new keymap is a new state that starts with none of them. Smithay
//! carries over the keys held down and nothing else, so a save that changed
//! only the repeat rate used to put a Russian typist back in English with
//! Caps Lock off. [`Compositor::set_keymap`] puts all three back.
//!
//! Under `switching = "window"` each window keeps its own layout
//! ([`perspicax_policy::LayoutMemory`]), switched to when it takes the
//! keyboard.
//!
//! Putting the locks back is quiet in smithay: `set_modifier_state` changes
//! the state without a `modifiers` event, so the focused client would go on
//! reading keys without Caps Lock, and without a word to the lights. Both are
//! sent here by hand.

use std::sync::PoisonError;

use perspicax_node::SurfaceId;
use smithay::{
    input::keyboard::{Error, KeyboardTarget as _, Layout, XkbConfig},
    utils::SERIAL_COUNTER,
};

use crate::{act::Keys, state::Compositor};

/// A keymap by xkb's RMLVO names: what `[input.keyboard]` names on a seat,
/// and [`crate::Command::Keymap`] headless. An empty name is xkb's own
/// default, which `XKB_DEFAULT_LAYOUT` and friends then decide. Several
/// layouts are one comma-separated name, as in "us,ru".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keymap {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
}

impl Keymap {
    /// The same names, as smithay takes them.
    pub(crate) fn xkb(&self) -> XkbConfig<'_> {
        XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: self.options.clone(),
        }
    }
}

impl Compositor {
    /// Give the seat's keyboard a new keymap, and the table agents type from
    /// the same one, keeping the layout in use, Caps Lock and Num Lock.
    ///
    /// The layout is kept by its number, so "us,ru" in Russian becomes
    /// "us,ru,de" in Russian, and a keymap with fewer layouts keeps its last.
    /// `numlock` sets Num Lock rather than keeping it.
    ///
    /// # Errors
    ///
    /// xkb could not compile the names. The keyboard keeps its old keymap.
    pub(crate) fn set_keymap(
        &mut self,
        keymap: &Keymap,
        numlock: Option<bool>,
    ) -> Result<(), Error> {
        let Some(keyboard) = self.keyboard.clone() else {
            return Ok(());
        };
        let before = keyboard.modifier_state();
        let layout = self.layouts().map_or(0, |(active, _)| active);
        keyboard.set_xkb_config(self, keymap.xkb())?;
        // Only the locks are carried over: keys held down are already down
        // in the new state, pressed again by smithay.
        let mut locks = keyboard.modifier_state();
        locks.caps_lock = before.caps_lock;
        locks.num_lock = numlock.unwrap_or(before.num_lock);
        // This also puts the layout back to the first, so it goes first.
        keyboard.set_modifier_state(locks);
        if let Some((_, count)) = self.layouts() {
            self.lock_layout(layout.min(count - 1));
        }
        if let Some(focus) = keyboard.current_focus() {
            let seat = self.seat.clone();
            let modifiers = keyboard.modifier_state();
            focus.modifiers(&seat, self, modifiers, SERIAL_COUNTER.next_serial());
        }
        self.backend.light(keyboard.led_state());
        self.keys = Keys::new(keymap);
        Ok(())
    }

    /// The layout in use, counted from 0, and how many the keymap has.
    pub(crate) fn layouts(&mut self) -> Option<(u32, u32)> {
        let keyboard = self.keyboard.clone()?;
        keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap_or_else(PoisonError::into_inner);
            let count = xkb.layouts().last().map_or(1, |last| last.0 + 1);
            Some((xkb.active_layout().0, count))
        })
    }

    /// Lock the keyboard into a layout, counted from 0. The focused client
    /// is told by smithay, which sends the new group with the modifiers,
    /// and the lights by `led_state_changed`, which smithay calls.
    pub(crate) fn lock_layout(&mut self, layout: u32) {
        let Some(keyboard) = self.keyboard.clone() else {
            return;
        };
        keyboard.with_xkb_state(self, |mut context| context.set_layout(Layout(layout)));
    }

    /// Switch to the next layout, or the previous, wrapping round.
    pub(crate) fn cycle_layout(&mut self, forward: bool) {
        if let Some((active, count)) = self.layouts() {
            let step = if forward { 1 } else { count - 1 };
            self.lock_layout((active + step) % count);
        }
    }

    /// Switch to the layout a person calls this number, counting from 1.
    /// A number past the last layout does nothing.
    pub(crate) fn go_to_layout(&mut self, number: u8) {
        let Some(index) = u32::from(number).checked_sub(1) else {
            return;
        };
        if self.layouts().is_some_and(|(_, count)| index < count) {
            self.lock_layout(index);
        }
    }

    /// The keyboard went to `window`, or to something that is not a window:
    /// when each window has its own layout, remember the one the last window
    /// was typed in, and switch to this one's.
    pub(crate) fn follow_layout(&mut self, window: Option<SurfaceId>) {
        let Some((active, count)) = self.layouts() else {
            return;
        };
        if let Some(layout) = self.layout_memory.focus(window, active) {
            // Kept from a keymap that had more layouts than this one.
            self.lock_layout(layout.min(count - 1));
        }
    }
}
