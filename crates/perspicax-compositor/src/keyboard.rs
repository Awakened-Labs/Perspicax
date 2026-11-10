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
//! keyboard: before anything next reads, switches or types in the layout,
//! so a switch made just after is that window's (#237).
//!
//! A keymap is names, and xkb is first asked about them when it compiles
//! them. [`Keymap::check`] asks it without the keyboard, so a name it lacks
//! is refused where the config is judged, naming the key that wrote it,
//! rather than only logged when the keyboard is given it (#215). Some names
//! never reach xkb at all: one holding a NUL, and a blank layout in a list.
//!
//! Putting the locks back is quiet in smithay: `set_modifier_state` changes
//! the state without a `modifiers` event, so the focused client would go on
//! reading keys without Caps Lock, and without a word to the lights. Both are
//! sent here by hand.
//!
//! Every key goes out through [`Compositor::send_key`]: a person's at the
//! seat, a stand-in's for a test, an agent's. While the session is locked it
//! goes to the lock screen, whatever the keyboard was given to on the way
//! (#197). Each way of giving it asks whether the lock allows it (#127), but
//! that is a list, and a way it misses is the password typed into a window
//! behind the lock, Enter and all. So the keyboard is put back on the lock
//! screen as each key goes, and what it reaches is fenced as it is sent
//! (`crate::focus`). Smithay's own ways out, `KeyboardHandle::input` and the
//! halves it is made of, are refused by clippy everywhere else.

use std::sync::PoisonError;

use smithay::{
    backend::input::KeyState,
    input::keyboard::{
        Error, FilterResult, KeyboardHandle, KeyboardTarget as _, Keycode, KeysymHandle, Layout,
        ModifiersState, XkbConfig, xkb,
    },
    utils::{SERIAL_COUNTER, Serial},
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

    /// Whether xkb compiles this keymap, as the seat's keyboard would: asked
    /// without the keyboard, so a config is judged by it before the keyboard
    /// is given it (#215).
    ///
    /// # Errors
    ///
    /// The `[input.keyboard]` key that writes what xkb lacks, and why. xkb
    /// says why only in its log, as the paths it searched, so the keymap's
    /// parts are compiled one at a time instead: the rules, then each layout
    /// and its variant. A model or an option xkb does not know never fails
    /// it: xkb drops the name and compiles the rest. Only a broken file of
    /// its own fails there, and that is said of the keymap as a whole.
    pub fn check(&self) -> Result<(), Uncompiled> {
        self.readable()?;
        let mut context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // Its words are said in the key's instead, and `config check`'s are
        // shown whole.
        context.set_log_level(xkb::LogLevel::Critical);
        let compiles = |keymap: &Self| {
            xkb::Keymap::new_from_names(
                &context,
                &keymap.rules,
                &keymap.model,
                &keymap.layout,
                &keymap.variant,
                keymap.options.clone(),
                xkb::COMPILE_NO_FLAGS,
            )
            .is_some()
        };
        if compiles(self) {
            return Ok(());
        }
        Err(self.fault(compiles))
    }

    /// Which part of this keymap, which xkb does not compile, it fails on:
    /// the first that fails alone, or else the whole.
    fn fault(&self, compiles: impl Fn(&Self) -> bool) -> Uncompiled {
        let alone = |layout: &str, variant: &str| Self {
            rules: self.rules.clone(),
            layout: layout.to_owned(),
            variant: variant.to_owned(),
            ..Self::default()
        };
        if !self.rules.is_empty() && !compiles(&alone("", "")) {
            return Uncompiled::new(
                "input.keyboard.rules",
                format!("xkb has no rules “{}”", self.rules),
            );
        }
        let mut variants = self.variant.split(',');
        for layout in self.layout.split(',') {
            let variant = variants.next().unwrap_or_default();
            let (layout, variant) = (layout.trim(), variant.trim());
            if !compiles(&alone(layout, "")) {
                return Uncompiled::new(
                    "input.keyboard.layout",
                    format!("xkb has no layout “{layout}”"),
                );
            }
            if !variant.is_empty() && !compiles(&alone(layout, variant)) {
                return Uncompiled::new(
                    "input.keyboard.variant",
                    format!("xkb's layout “{layout}” has no variant “{variant}”"),
                );
            }
        }
        Uncompiled::new(
            "input.keyboard",
            "xkb cannot compile this keymap".to_owned(),
        )
    }

    /// Whether xkbcommon may be handed these names at all. Its Rust binding
    /// panics on a NUL in one, which would end the session rather than
    /// refuse a save. A blank name in a layout list, as "us,,ru", is a typo
    /// xkb never says it took: it fails on a leading one, drops a trailing
    /// one, and makes one between two a layout with no name and the first
    /// layout's keys, which the panel would list blank (#215).
    fn readable(&self) -> Result<(), Uncompiled> {
        let names = [
            ("input.keyboard.rules", self.rules.as_str()),
            ("input.keyboard.model", &self.model),
            ("input.keyboard.layout", &self.layout),
            ("input.keyboard.variant", &self.variant),
            (
                "input.keyboard.options",
                self.options.as_deref().unwrap_or_default(),
            ),
        ];
        if let Some((key, _)) = names.into_iter().find(|(_, name)| name.contains('\0')) {
            return Err(Uncompiled::new(
                key,
                "holds a NUL character, which xkb cannot read".to_owned(),
            ));
        }
        let mut layouts = self.layout.split(',');
        if self.layout.contains(',') && layouts.any(|name| name.trim().is_empty()) {
            return Err(Uncompiled::new(
                "input.keyboard.layout",
                format!("“{}” has a layout with no name", self.layout),
            ));
        }
        Ok(())
    }
}

/// Why xkb cannot compile a [`Keymap`]: the `[input.keyboard]` key that
/// writes what it lacks, and why, said as the config says a value it cannot
/// use, so a refusal reads the same whichever of them found it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{key}`: {reason}")]
pub struct Uncompiled {
    /// The key, as `input.keyboard.layout`; `input.keyboard` for the
    /// keymap as a whole.
    pub key: &'static str,
    /// Why, naming what xkb lacks as the file writes it.
    pub reason: String,
}

impl Uncompiled {
    fn new(key: &'static str, reason: String) -> Self {
        Self { key, reason }
    }
}

impl Compositor {
    /// Send one key to whatever has the keyboard, unless `filter`, which
    /// sees it first, keeps it: a binding or an escape hatch. What `filter`
    /// keeps is returned, and the key goes nowhere.
    ///
    /// While the session is locked, a key that would go anywhere but the lock
    /// screen is a way to the keyboard the lock's gate missed: it is said,
    /// and the keyboard is put back before the key goes (#197). Between the
    /// filter and the sending, so nothing can move it in between.
    ///
    /// The key is pressed in the layout of what has the keyboard: one that
    /// went to a window in this turn has its layout first, so the first key
    /// typed there, or xkb's own switch, is not in the last window's (#237).
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place a key goes out, kept on the lock screen while locked (#197)"
    )]
    pub(crate) fn send_key<T>(
        &mut self,
        keyboard: &KeyboardHandle<Self>,
        keycode: Keycode,
        state: KeyState,
        serial: Serial,
        time: u32,
        filter: impl FnOnce(&mut Self, &ModifiersState, KeysymHandle<'_>) -> FilterResult<T>,
    ) -> Option<T> {
        self.follow_layout();
        let (filtered, mods_changed) = keyboard.input_intercept(self, keycode, state, filter);
        if let FilterResult::Intercept(kept) = filtered {
            return Some(kept);
        }
        if !self.keys_with_the_lock() {
            tracing::warn!(
                origin = ?keyboard.current_focus().as_ref().map(crate::lock::origin_of),
                grabbed = keyboard.is_grabbed(),
                "while locked, a key was going to what is not the lock screen: the lock screen has the keys again"
            );
            self.keep_keys_on_lock();
        }
        keyboard.input_forward(self, keycode, state, serial, time, mods_changed);
        None
    }

    /// Give the seat's keyboard a new keymap, and the table agents type from
    /// the same one, keeping the layout in use, Caps Lock and Num Lock.
    ///
    /// The layout is kept by its number, so "us,ru" in Russian becomes
    /// "us,ru,de" in Russian, and a keymap with fewer layouts keeps its last.
    /// `numlock` sets Num Lock rather than keeping it.
    ///
    /// # Errors
    ///
    /// xkb could not compile the names, or could not be handed them (see
    /// [`Keymap::check`]). The keyboard keeps its old keymap.
    pub(crate) fn set_keymap(
        &mut self,
        keymap: &Keymap,
        numlock: Option<bool>,
    ) -> Result<(), Error> {
        let Some(keyboard) = self.keyboard.clone() else {
            return Ok(());
        };
        keymap.readable().map_err(|_| Error::BadKeymap)?;
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
        self.keymap = keymap.clone();
        Ok(())
    }

    /// The keymap's layouts as a shell shows them: xkb's name for each, as
    /// "English (US)", and a short label for a panel.
    pub(crate) fn layout_labels(&mut self) -> Vec<(String, String)> {
        let Some(keyboard) = self.keyboard.clone() else {
            return Vec::new();
        };
        let names: Vec<String> = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap_or_else(PoisonError::into_inner);
            xkb.layouts()
                .map(|layout| xkb.layout_name(layout).to_owned())
                .collect()
        });
        let mut written = self.keymap.layout.split(',');
        names
            .into_iter()
            .map(|name| {
                let short = short(written.next(), &name);
                (name, short)
            })
            .collect()
    }

    /// The layout in use, counted from 0, and how many the keymap has, once
    /// the layout has followed the keyboard ([`Self::follow_layout`]).
    pub(crate) fn layouts(&mut self) -> Option<(u32, u32)> {
        self.follow_layout();
        self.xkb_layouts()
    }

    /// The same, as xkb has them, without following the keyboard first:
    /// for [`Self::follow_layout`].
    fn xkb_layouts(&mut self) -> Option<(u32, u32)> {
        let keyboard = self.keyboard.clone()?;
        keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap_or_else(PoisonError::into_inner);
            let count = xkb.layouts().last().map_or(1, |last| last.0 + 1);
            Some((xkb.active_layout().0, count))
        })
    }

    /// Lock the keyboard into a layout, counted from 0, once the layout has
    /// followed the keyboard: a switch made after it moved is the window's
    /// it went to. The focused client is told by smithay, which sends the
    /// new group with the modifiers, and the lights by `led_state_changed`,
    /// which smithay calls.
    pub(crate) fn lock_layout(&mut self, layout: u32) {
        self.follow_layout();
        self.xkb_lock_layout(layout);
    }

    /// The same, without following the keyboard first: for
    /// [`Self::follow_layout`].
    fn xkb_lock_layout(&mut self, layout: u32) {
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

    /// When each window has its own layout and the keyboard has moved since
    /// the layout last followed it: remember the layout in use as the one
    /// the window it left was typed in, and switch to the one it went to's.
    ///
    /// `focus_changed` only notes the move, as smithay calls it holding the
    /// keyboard. It is followed here before anything reads, switches or
    /// types in the layout, and otherwise once the loop is idle. Only an
    /// idle used to follow it, after whatever else the turn had done: a
    /// switch made in the turn the keyboard moved was undone, and remembered
    /// as the layout of the window it left (#237).
    pub(crate) fn follow_layout(&mut self) {
        if !self.layout_memory.has_moved() {
            return;
        }
        let Some((active, count)) = self.xkb_layouts() else {
            return;
        };
        if let Some(layout) = self.layout_memory.follow(active) {
            // Kept from a keymap that had more layouts than this one.
            self.xkb_lock_layout(layout.min(count - 1));
        }
    }
}

/// A layout's label for a panel: its name as the config wrote it, in
/// capitals, so "us,ru" gives "US" and "RU", or with none written, the first
/// two letters of xkb's own name for it.
fn short(written: Option<&str>, name: &str) -> String {
    match written.map(str::trim).filter(|written| !written.is_empty()) {
        Some(written) => written.to_uppercase(),
        None => name.chars().take(2).collect::<String>().to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap(layout: &str, variant: &str) -> Keymap {
        Keymap {
            layout: layout.to_owned(),
            variant: variant.to_owned(),
            ..Keymap::default()
        }
    }

    /// What `check` says of `keymap`, as the config would show it.
    fn refused(keymap: &Keymap) -> String {
        keymap.check().expect_err("refused").to_string()
    }

    #[test]
    fn a_keymap_xkb_knows_is_checked_and_taken() {
        let keymap = Keymap {
            options: Some("ctrl:nocaps,grp:alt_shift_toggle".to_owned()),
            ..keymap("us,ru", ",phonetic")
        };
        assert_eq!(keymap.check(), Ok(()));
        assert_eq!(Keymap::default().check(), Ok(()), "xkb's own default");
    }

    #[test]
    fn a_layout_xkb_lacks_is_refused_naming_it_and_not_the_others() {
        assert_eq!(
            refused(&keymap("us,rux", "")),
            "`input.keyboard.layout`: xkb has no layout “rux”"
        );
    }

    #[test]
    fn a_variant_its_layout_lacks_is_refused_naming_both() {
        assert_eq!(
            refused(&keymap("us,ru", ",nope")),
            "`input.keyboard.variant`: xkb's layout “ru” has no variant “nope”"
        );
    }

    #[test]
    fn rules_xkb_lacks_are_refused_before_the_layouts_read_through_them() {
        let keymap = Keymap {
            rules: "nope".to_owned(),
            ..keymap("us", "")
        };
        assert_eq!(
            refused(&keymap),
            "`input.keyboard.rules`: xkb has no rules “nope”"
        );
    }

    #[test]
    fn a_name_holding_a_nul_is_refused_without_asking_xkb() {
        // xkbcommon's binding would panic on it, ending the session.
        assert_eq!(
            refused(&keymap("us\0", "")),
            "`input.keyboard.layout`: holds a NUL character, which xkb cannot read"
        );
        let options = Keymap {
            options: Some("ctrl:nocaps\0".to_owned()),
            ..Keymap::default()
        };
        assert!(refused(&options).starts_with("`input.keyboard.options`"));
    }

    #[test]
    fn a_layout_with_no_name_among_others_is_refused_without_asking_xkb() {
        // xkb makes the blank in the first two a nameless copy of “us”,
        // fails on the third, and drops the fourth's without a word.
        for written in ["us,,ru", "us, ,ru", ",us", "us,ru,"] {
            assert_eq!(
                refused(&keymap(written, "")),
                format!("`input.keyboard.layout`: “{written}” has a layout with no name"),
            );
        }
    }

    #[test]
    fn a_layout_is_labelled_as_the_config_wrote_it_or_by_xkbs_name() {
        assert_eq!(short(Some("us"), "English (US)"), "US");
        assert_eq!(short(Some(" ru"), "Russian"), "RU");
        assert_eq!(short(None, "English (US)"), "EN", "xkb's default");
        assert_eq!(short(Some(""), "Russian"), "RU");
    }
}
