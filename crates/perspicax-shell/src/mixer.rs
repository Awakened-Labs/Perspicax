//! What the mixer does when something happens: opening it, the pointer,
//! the buttons, the wheel, a key, the sound changing.
//!
//! A reducer, as [`crate::update`] is for the menus and [`crate::pie`] for
//! a pie: an [`Event`] and the [`State`] in, the state changed and the
//! [`Effect`]s to carry out back, tested as values.
//!
//! One mixer is open at a time, beside the panel's mixer item. A press on a
//! mute button mutes, or brings the sound back; a press on a slider sets
//! the volume where it is pressed, and a drag from there follows the
//! pointer, the slider showing where it was dragged to until the sound
//! server says it is there; a press on a device listed moves its side to
//! it; a press on a side's fold unfolds what has nothing plugged in, or
//! folds it away again. The wheel over a slider or its mute button steps
//! the slider, and anywhere else on the mixer scrolls it, if it is too tall
//! for the room it has. A press off the mixer closes it, as do Escape and
//! losing the keyboard.
//!
//! The keyboard moves through the controls with Tab and Shift+Tab, or the
//! arrows up and down, scrolling each into sight. On a slider, left and
//! right step it, Page Up and Page Down move it ten at a time, and Home and
//! End take it to silence or to the limit; on anything else, Space or
//! Return presses it. The control the keyboard is on is ringed once a key
//! is used, and not while the pointer is what is used.

use crate::{
    layout::mixer::{Anchor, Control, Mixed, Placed, lay_out, percent_at},
    model::{
        Button,
        sound::{Change, Side, stepped},
    },
};

/// How far Page Up and Page Down move a slider, in percent.
const PAGE: u32 = 10;
/// How far a notch of the wheel scrolls the mixer, in pixels.
const NOTCH: i32 = 40;

/// A key pressed while the mixer has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Escape,
    /// The next control, or with Shift, the one before.
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    /// Return or Space.
    Press,
}

/// What an assistive technology asks of a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    Click,
    Increment,
    Decrement,
    /// Set a slider to this, in percent.
    Set(u32),
    Focus,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    /// The mixer was asked for on the monitor named `output`, to open at
    /// `anchor`: open it, showing `mixed`.
    Open {
        output: String,
        anchor: Anchor,
        mixed: Mixed,
    },
    /// Close it: asked for again, or a press elsewhere on the panel.
    Close,
    /// The sound is now as `mixed` says.
    Heard(Mixed),
    /// The pointer came onto the mixer's surface, or moved on it.
    Motion((f64, f64)),
    Press((f64, f64), Button),
    Release((f64, f64), Button),
    /// The wheel turned this many notches, up negative, at the pointer.
    Wheel(i32),
    Key(Key),
    /// The mixer's surface lost the keyboard to something else.
    KeyboardLost,
    /// An assistive technology asked `act` of `control`.
    Act(Control, Act),
}

/// What to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// What the mixer shows changed: draw it again, or take it away.
    Redraw,
    /// Mute the default on this side, or bring its sound back.
    Mute(Side),
    /// Change the volume of the default on this side.
    Volume(Side, Change),
    /// Move this side to the device with this key.
    Choose(Side, String),
}

/// The mixer, if it is open.
#[derive(Debug, Default)]
pub(crate) struct State {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    output: String,
    anchor: Anchor,
    placed: Placed,
    mixed: Mixed,
    /// Whether each side's devices with nothing plugged in are unfolded.
    unfolded: [bool; 2],
    /// The control the keyboard is on, and whether a key has been used
    /// since the pointer was, so that it is shown.
    focus: Control,
    keyed: bool,
    /// Where the pointer is on the surface, once it is known.
    pointer: Option<(f64, f64)>,
    /// The slider being dragged, and the volume it was dragged to: shown
    /// in place of the server's until the server says it.
    dragging: Option<Side>,
    dragged: Option<(Side, u32)>,
}

/// The mixer as it is to be drawn.
#[derive(Debug)]
pub(crate) struct View<'a> {
    /// The monitor it is open on, by connector name.
    pub(crate) output: &'a str,
    pub(crate) placed: &'a Placed,
    pub(crate) mixed: &'a Mixed,
    pub(crate) focus: Control,
    /// The control to ring: the keyboard's, once a key is used.
    pub(crate) ring: Option<Control>,
    /// What each side's slider shows: a volume dragged to, or the
    /// server's.
    pub(crate) shown: [Option<u32>; 2],
}

impl State {
    /// Whether the mixer is open, and on which monitor.
    pub(crate) fn open_on(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.output.as_str())
    }

    /// The mixer as it is to be drawn, if it is open.
    pub(crate) fn view(&self) -> Option<View<'_>> {
        let open = self.open.as_ref()?;
        let shown = [Side::Output, Side::Input].map(|side| open.shown(side));
        Some(View {
            output: &open.output,
            placed: &open.placed,
            mixed: &open.mixed,
            focus: open.focus,
            ring: open.keyed.then_some(open.focus),
            shown,
        })
    }

    /// Handle `event`.
    pub(crate) fn update(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Open {
                output,
                anchor,
                mixed,
            } => {
                let unfolded = [false; 2];
                self.open = Some(Open {
                    output,
                    anchor,
                    placed: lay_out(&anchor, &mixed, unfolded, 0),
                    mixed,
                    unfolded,
                    focus: Control::Volume(Side::Output),
                    keyed: false,
                    pointer: None,
                    dragging: None,
                    dragged: None,
                });
                vec![Effect::Redraw]
            }
            Event::Close | Event::KeyboardLost => self.close(),
            Event::Key(Key::Escape) => self.close(),
            event => {
                let Some(open) = &mut self.open else {
                    return Vec::new();
                };
                match open.update(event) {
                    Some(effects) => effects,
                    None => self.close(),
                }
            }
        }
    }

    fn close(&mut self) -> Vec<Effect> {
        match self.open.take() {
            Some(_) => vec![Effect::Redraw],
            None => Vec::new(),
        }
    }
}

impl Open {
    /// What `side`'s slider shows.
    fn shown(&self, side: Side) -> Option<u32> {
        match self.dragged {
            Some((dragged, percent)) if dragged == side => Some(percent),
            _ => self.mixed.on(side).map(|master| master.level.percent),
        }
    }

    /// Handle `event` while open: what to do, or `None` to close.
    fn update(&mut self, event: Event) -> Option<Vec<Effect>> {
        Some(match event {
            Event::Heard(mixed) => {
                // A volume dragged to stands until the server says it: one
                // it says on the way, before the drag ends, is set again by
                // the drag's next move.
                self.dragged.take_if(|&mut (side, percent)| {
                    mixed
                        .on(side)
                        .is_none_or(|master| master.level.percent == percent)
                });
                self.mixed = mixed;
                self.lay_out(self.placed.scroll);
                vec![Effect::Redraw]
            }
            Event::Motion(at) => {
                self.pointer = Some(at);
                match self.dragging {
                    Some(side) => self.set_at(side, at.0),
                    None => Vec::new(),
                }
            }
            Event::Press(at, Button::Left) => {
                self.pointer = Some(at);
                if !self.placed.rect.contains(at) {
                    return None;
                }
                let ringed = std::mem::replace(&mut self.keyed, false);
                let mut effects = match self.placed.at(at) {
                    Some(Control::Volume(side)) => {
                        self.focus = Control::Volume(side);
                        self.dragging = Some(side);
                        let mut effects = self.set_at(side, at.0);
                        effects.push(Effect::Redraw);
                        effects
                    }
                    Some(control) => {
                        self.focus = control;
                        self.press(control)
                    }
                    None => Vec::new(),
                };
                if ringed && !effects.contains(&Effect::Redraw) {
                    effects.push(Effect::Redraw);
                }
                effects
            }
            Event::Press(at, _) => {
                if self.placed.rect.contains(at) {
                    Vec::new()
                } else {
                    return None;
                }
            }
            Event::Release(_, Button::Left) => {
                self.dragging = None;
                Vec::new()
            }
            Event::Release(..) => Vec::new(),
            Event::Wheel(notches) => match self.pointer {
                Some(at) => match self.placed.at(at) {
                    Some(Control::Volume(side) | Control::Mute(side)) => self.step(side, -notches),
                    _ if self.placed.rect.contains(at) => self.scroll_by(notches * NOTCH),
                    _ => Vec::new(),
                },
                None => Vec::new(),
            },
            Event::Key(key) => {
                let shown = !std::mem::replace(&mut self.keyed, true);
                let mut effects = self.key(key);
                if shown && !effects.contains(&Effect::Redraw) {
                    effects.push(Effect::Redraw);
                }
                effects
            }
            Event::Act(control, act) => self.act(control, act),
            Event::Open { .. } | Event::Close | Event::KeyboardLost => Vec::new(),
        })
    }

    /// Lay the mixer out again as it now is, scrolled `scroll` down, or as
    /// near as it goes. The keyboard stays on its control, or if that has
    /// gone, moves to the one now where it was.
    fn lay_out(&mut self, scroll: i32) {
        let was = self
            .placed
            .controls
            .iter()
            .position(|&(control, _)| control == self.focus);
        self.placed = lay_out(&self.anchor, &self.mixed, self.unfolded, scroll);
        let controls = &self.placed.controls;
        if self.placed.of(self.focus).is_none()
            && let Some(&(control, _)) = was.and_then(|at| controls.get(at).or(controls.last()))
        {
            self.focus = control;
        }
    }

    /// Scroll so that the control the keyboard is on is in sight.
    fn reveal(&mut self) {
        let scroll = self.placed.reveal(self.focus);
        if scroll != self.placed.scroll {
            self.lay_out(scroll);
        }
    }

    /// Scroll `by` pixels, down positive, as far as it goes.
    fn scroll_by(&mut self, by: i32) -> Vec<Effect> {
        let was = self.placed.scroll;
        self.lay_out(was + by);
        if self.placed.scroll == was {
            Vec::new()
        } else {
            vec![Effect::Redraw]
        }
    }

    /// Set `side`'s volume where `x` is along its slider.
    fn set_at(&mut self, side: Side, x: f64) -> Vec<Effect> {
        let Some(slider) = self.placed.of(Control::Volume(side)) else {
            return Vec::new();
        };
        let percent = percent_at(slider, x, self.mixed.most(side));
        self.set(side, percent)
    }

    /// Set `side`'s volume to `percent`, if it has a device and that is a
    /// change.
    fn set(&mut self, side: Side, percent: u32) -> Vec<Effect> {
        if self.mixed.on(side).is_none() || self.shown(side) == Some(percent) {
            return Vec::new();
        }
        self.dragged = Some((side, percent));
        vec![Effect::Volume(side, Change::To(percent)), Effect::Redraw]
    }

    /// Step `side`'s volume `notches`, up positive.
    fn step(&mut self, side: Side, notches: i32) -> Vec<Effect> {
        let Some(now) = self.shown(side) else {
            return Vec::new();
        };
        let to = stepped(now, notches, self.mixed.step, self.mixed.limit);
        self.set(side, to)
    }

    /// Press `control`, as a click, Return or an assistive technology does:
    /// a slider is not pressed.
    fn press(&mut self, control: Control) -> Vec<Effect> {
        match control {
            Control::Mute(side) => match self.mixed.on(side) {
                Some(_) => vec![Effect::Mute(side)],
                None => Vec::new(),
            },
            Control::Volume(_) => Vec::new(),
            Control::Choose(side, number) => match self.mixed.find(side, number) {
                Some(listed) if !listed.chosen => vec![Effect::Choose(side, listed.key.clone())],
                _ => Vec::new(),
            },
            Control::Fold(side) => {
                self.unfolded[side.at()] = !self.unfolded[side.at()];
                self.lay_out(self.placed.scroll);
                vec![Effect::Redraw]
            }
        }
    }

    /// The control `by` places on from the focused one, round the ends,
    /// scrolled into sight.
    fn move_focus(&mut self, by: isize) -> Vec<Effect> {
        let all = &self.placed.controls;
        let at = all
            .iter()
            .position(|&(control, _)| control == self.focus)
            .unwrap_or(0);
        let next = (at as isize + by).rem_euclid(all.len() as isize) as usize;
        self.focus = all[next].0;
        self.reveal();
        vec![Effect::Redraw]
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        let slider = match self.focus {
            Control::Volume(side) => Some(side),
            _ => None,
        };
        match (key, slider) {
            (Key::Tab | Key::Down, _) => self.move_focus(1),
            (Key::BackTab | Key::Up, _) => self.move_focus(-1),
            (Key::Right, Some(side)) => self.step(side, 1),
            (Key::Left, Some(side)) => self.step(side, -1),
            (Key::PageUp, Some(side)) => {
                let most = self.mixed.most(side);
                let to = self.shown(side).map(|now| (now + PAGE).min(most));
                to.map_or_else(Vec::new, |to| self.set(side, to))
            }
            (Key::PageDown, Some(side)) => {
                let to = self.shown(side).map(|now| now.saturating_sub(PAGE));
                to.map_or_else(Vec::new, |to| self.set(side, to))
            }
            (Key::Home, Some(side)) => self.set(side, 0),
            (Key::End, Some(side)) => {
                let limit = self.mixed.limit;
                self.set(side, limit)
            }
            (Key::Press, None) => self.press(self.focus),
            _ => Vec::new(),
        }
    }

    fn act(&mut self, control: Control, act: Act) -> Vec<Effect> {
        if self.placed.of(control).is_none() {
            return Vec::new();
        }
        match (control, act) {
            (Control::Volume(side), Act::Increment) => self.step(side, 1),
            (Control::Volume(side), Act::Decrement) => self.step(side, -1),
            (Control::Volume(side), Act::Set(percent)) => self.set(side, percent),
            (control, Act::Click) => self.press(control),
            (control, Act::Focus) => {
                self.focus = control;
                self.reveal();
                vec![Effect::Redraw]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use perspicax_config::Edge;

    use super::*;
    use crate::{
        layout::{
            Rect,
            mixer::{Listed, Master, number},
        },
        model::sound::{Kind, Level},
    };

    const ANCHOR: Anchor = Anchor {
        button: Rect::new(1180, 760, 100, 40),
        edge: Edge::Bottom,
        area: Rect::new(0, 0, 1280, 760),
    };

    fn mixed(output: u32, input: Option<u32>) -> Mixed {
        let master = |name: &str, percent| Master {
            name: name.to_owned(),
            level: Level {
                percent,
                muted: false,
            },
        };
        Mixed {
            output: Some(master("Speakers", output)),
            input: input.map(|percent| master("Microphone", percent)),
            step: 5,
            limit: 100,
            ..Mixed::default()
        }
    }

    /// An output listed: plugged in, or not, and chosen, or not.
    fn listed(key: &str, connected: bool, chosen: bool) -> Listed {
        Listed {
            key: key.to_owned(),
            device: key.to_owned(),
            jack: None,
            kind: Kind::Speaker,
            connected,
            chosen,
        }
    }

    /// Speakers with the sound on them, a headset plugged in, and two
    /// jacks with nothing in them.
    fn laptop() -> Mixed {
        Mixed {
            outputs: vec![
                listed("speakers", true, true),
                listed("headphones", false, false),
                listed("hdmi", false, false),
                listed("headset", true, false),
            ],
            ..mixed(40, Some(60))
        }
    }

    fn opened(mixed: Mixed) -> State {
        opened_at(ANCHOR, mixed)
    }

    fn opened_at(anchor: Anchor, mixed: Mixed) -> State {
        let mut state = State::default();
        let effects = state.update(Event::Open {
            output: "DP-1".to_owned(),
            anchor,
            mixed,
        });
        assert_eq!(effects, [Effect::Redraw]);
        state
    }

    fn row(key: &str) -> Control {
        Control::Choose(Side::Output, number(key))
    }

    fn controls(state: &State) -> Vec<Control> {
        let view = state.view().expect("open");
        view.placed
            .controls
            .iter()
            .map(|&(control, _)| control)
            .collect()
    }

    /// A point on `control`, a fraction `along` of its width from its left.
    fn on(state: &State, control: Control, along: f64) -> (f64, f64) {
        let rect = state
            .view()
            .expect("open")
            .placed
            .of(control)
            .expect("there");
        (
            f64::from(rect.x) + along * f64::from(rect.w),
            f64::from(rect.y) + 4.0,
        )
    }

    /// The volume `effects` ask for on `side`.
    fn asked(effects: &[Effect], side: Side) -> Option<Change> {
        effects.iter().find_map(|effect| match effect {
            Effect::Volume(on, change) if *on == side => Some(*change),
            _ => None,
        })
    }

    #[test]
    fn a_press_on_a_mute_button_mutes_that_side() {
        let mut state = opened(mixed(40, Some(60)));
        let at = on(&state, Control::Mute(Side::Input), 0.5);
        assert_eq!(
            state.update(Event::Press(at, Button::Left)),
            [Effect::Mute(Side::Input)]
        );
        let mut none = opened(mixed(40, None));
        let at = on(&none, Control::Mute(Side::Input), 0.5);
        assert_eq!(
            none.update(Event::Press(at, Button::Left)),
            [],
            "no input to mute"
        );
    }

    #[test]
    fn a_drag_sets_the_volume_where_the_pointer_is_and_shows_it_until_heard() {
        let mut state = opened(mixed(40, Some(60)));
        let slider = Control::Volume(Side::Output);
        let effects = state.update(Event::Press(on(&state, slider, 0.99), Button::Left));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(100)));
        let effects = state.update(Event::Motion(on(&state, slider, 0.0)));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(0)));
        assert_eq!(
            state.view().expect("open").shown[0],
            Some(0),
            "where it was dragged"
        );
        // The server says 40 still: the drag stands while it goes on.
        state.update(Event::Heard(mixed(40, Some(60))));
        assert_eq!(state.view().expect("open").shown[0], Some(0));
        state.update(Event::Release(on(&state, slider, 0.0), Button::Left));
        assert!(
            state
                .update(Event::Motion(on(&state, slider, 0.5)))
                .is_empty(),
            "let go"
        );
        state.update(Event::Heard(mixed(0, Some(60))));
        assert_eq!(state.view().expect("open").shown[0], Some(0));
        state.update(Event::Heard(mixed(70, Some(60))));
        assert_eq!(
            state.view().expect("open").shown[0],
            Some(70),
            "the server's again"
        );
    }

    #[test]
    fn the_wheel_over_a_slider_steps_it() {
        let mut state = opened(mixed(24, Some(60)));
        state.update(Event::Motion(on(
            &state,
            Control::Volume(Side::Output),
            0.5,
        )));
        let effects = state.update(Event::Wheel(-1));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(25)));
        state.update(Event::Motion(on(&state, Control::Volume(Side::Input), 0.5)));
        let effects = state.update(Event::Wheel(1));
        assert_eq!(asked(&effects, Side::Input), Some(Change::To(55)));
    }

    #[test]
    fn page_up_raises_by_ten_never_past_the_limit_and_the_arrows_step() {
        let mut state = opened(mixed(95, Some(60)));
        assert_eq!(
            state.view().expect("open").focus,
            Control::Volume(Side::Output)
        );
        let effects = state.update(Event::Key(Key::PageUp));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(100)));
        let effects = state.update(Event::Key(Key::PageDown));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(90)));
        let effects = state.update(Event::Key(Key::Left));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(85)));
        let effects = state.update(Event::Key(Key::Home));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(0)));
        let effects = state.update(Event::Key(Key::End));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(100)));
    }

    #[test]
    fn tab_moves_through_the_controls_and_return_presses_a_button() {
        let mut state = opened(mixed(40, Some(60)));
        state.update(Event::Key(Key::Tab));
        assert_eq!(
            state.view().expect("open").focus,
            Control::Mute(Side::Input)
        );
        assert_eq!(
            state.update(Event::Key(Key::Press)),
            [Effect::Mute(Side::Input)]
        );
        state.update(Event::Key(Key::Tab));
        state.update(Event::Key(Key::Tab));
        assert_eq!(
            state.view().expect("open").focus,
            Control::Mute(Side::Output),
            "round"
        );
        state.update(Event::Key(Key::BackTab));
        assert_eq!(
            state.view().expect("open").focus,
            Control::Volume(Side::Input)
        );
        assert!(
            state.update(Event::Key(Key::Press)).is_empty(),
            "a slider is not pressed"
        );
    }

    #[test]
    fn the_keyboards_control_is_ringed_once_a_key_is_used_and_not_after_a_press() {
        let mut state = opened(mixed(40, Some(60)));
        assert_eq!(
            state.view().expect("open").ring,
            None,
            "opened by the pointer"
        );
        assert_eq!(state.update(Event::Key(Key::Up)), [Effect::Redraw]);
        assert_eq!(
            state.view().expect("open").ring,
            Some(Control::Mute(Side::Output))
        );
        let at = on(&state, Control::Volume(Side::Input), 0.5);
        state.update(Event::Press(at, Button::Left));
        assert_eq!(state.view().expect("open").ring, None);
        assert_eq!(
            state.view().expect("open").focus,
            Control::Volume(Side::Input)
        );
        state.update(Event::Key(Key::Up));
        let at = on(&state, Control::Mute(Side::Output), 0.5);
        assert_eq!(
            state.update(Event::Press(at, Button::Left)),
            [Effect::Mute(Side::Output), Effect::Redraw],
            "the ring taken away"
        );
    }

    #[test]
    fn escape_losing_the_keyboard_or_a_press_off_it_closes_it() {
        for close in [
            Event::Key(Key::Escape),
            Event::KeyboardLost,
            Event::Close,
            Event::Press((5.0, 5.0), Button::Left),
            Event::Press((5.0, 5.0), Button::Right),
        ] {
            let mut state = opened(mixed(40, Some(60)));
            assert_eq!(state.update(close.clone()), [Effect::Redraw], "{close:?}");
            assert!(state.view().is_none(), "{close:?}");
        }
        let mut state = opened(mixed(40, Some(60)));
        let inside = on(&state, Control::Volume(Side::Output), 0.5);
        state.update(Event::Press(inside, Button::Right));
        assert!(state.view().is_some(), "a right press on it leaves it open");
    }

    #[test]
    fn an_assistive_technology_clicks_steps_sets_and_focuses() {
        let mut state = opened(mixed(40, Some(60)));
        let output = Control::Volume(Side::Output);
        assert_eq!(
            state.update(Event::Act(Control::Mute(Side::Output), Act::Click)),
            [Effect::Mute(Side::Output)]
        );
        let effects = state.update(Event::Act(output, Act::Increment));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(45)));
        let effects = state.update(Event::Act(output, Act::Set(12)));
        assert_eq!(asked(&effects, Side::Output), Some(Change::To(12)));
        state.update(Event::Act(Control::Mute(Side::Input), Act::Focus));
        assert_eq!(
            state.view().expect("open").focus,
            Control::Mute(Side::Input)
        );
    }

    #[test]
    fn a_press_on_a_device_listed_moves_to_it_unless_the_sound_is_on_it() {
        let mut state = opened(laptop());
        let at = on(&state, row("headset"), 0.5);
        assert_eq!(
            state.update(Event::Press(at, Button::Left)),
            [Effect::Choose(Side::Output, "headset".to_owned())]
        );
        assert_eq!(state.view().expect("open").focus, row("headset"));
        let at = on(&state, row("speakers"), 0.5);
        assert!(
            state.update(Event::Press(at, Button::Left)).is_empty(),
            "the sound is on it"
        );
        state.update(Event::Key(Key::Down));
        assert_eq!(state.view().expect("open").focus, row("headset"));
        assert_eq!(
            state.update(Event::Key(Key::Press)),
            [Effect::Choose(Side::Output, "headset".to_owned())],
            "Return chooses it too"
        );
        assert_eq!(
            state.update(Event::Act(row("headset"), Act::Click)),
            [Effect::Choose(Side::Output, "headset".to_owned())]
        );
    }

    #[test]
    fn a_jack_with_nothing_in_it_is_folded_away_until_its_fold_is_pressed() {
        let mut state = opened(laptop());
        assert_eq!(
            controls(&state)[..5],
            [
                Control::Mute(Side::Output),
                Control::Volume(Side::Output),
                row("speakers"),
                row("headset"),
                Control::Fold(Side::Output),
            ],
            "what is plugged in, then the fold"
        );
        assert_eq!(state.view().expect("open").placed.folded, [2, 0]);
        let fold = on(&state, Control::Fold(Side::Output), 0.5);
        assert_eq!(
            state.update(Event::Press(fold, Button::Left)),
            [Effect::Redraw]
        );
        assert_eq!(
            controls(&state)[4..7],
            [Control::Fold(Side::Output), row("headphones"), row("hdmi")],
            "unfolded under it"
        );
        state.update(Event::Act(Control::Fold(Side::Output), Act::Click));
        assert!(!controls(&state).contains(&row("hdmi")), "folded again");
        assert!(
            state
                .update(Event::Act(Control::Fold(Side::Input), Act::Click))
                .is_empty(),
            "no input is folded away"
        );
        assert_eq!(state.view().expect("open").placed.unfolded, [false; 2]);
        let mut plugged = laptop();
        plugged.outputs[2].connected = true;
        state.update(Event::Heard(plugged));
        assert_eq!(
            controls(&state)[2..5],
            [row("speakers"), row("hdmi"), row("headset")],
            "a jack is listed once it is plugged in"
        );
    }

    #[test]
    fn a_device_that_goes_leaves_the_keyboard_on_the_control_now_where_it_was() {
        let mut state = opened(laptop());
        state.update(Event::Act(row("headset"), Act::Focus));
        let mut unplugged = laptop();
        unplugged.outputs.pop();
        state.update(Event::Heard(unplugged));
        assert_eq!(
            state.view().expect("open").focus,
            Control::Fold(Side::Output)
        );
    }

    /// The laptop's outputs in a mixer with room for little more than its
    /// outputs.
    fn cramped() -> State {
        let mut mixed = laptop();
        mixed
            .outputs
            .extend((0..6).map(|at| listed(&format!("usb {at}"), true, false)));
        opened_at(
            Anchor {
                area: Rect::new(0, 560, 1280, 200),
                ..ANCHOR
            },
            mixed,
        )
    }

    #[test]
    fn the_focused_control_is_scrolled_into_view() {
        let mut state = cramped();
        let view = state.view().expect("open");
        assert_eq!(view.placed.rect.h, 200, "cut to fit");
        assert!(view.placed.most > 0);
        state.update(Event::Key(Key::BackTab));
        let view = state.view().expect("open");
        assert_eq!(view.focus, Control::Mute(Side::Output));
        let back = Control::Volume(Side::Input);
        state.update(Event::Key(Key::BackTab));
        let view = state.view().expect("open");
        assert_eq!(view.focus, back, "round to the last");
        let rect = view.placed.of(back).expect("there");
        assert_eq!(rect.bottom(), view.placed.inside.bottom(), "just in sight");
        assert_eq!(view.placed.scroll, view.placed.most);
        state.update(Event::Key(Key::Tab));
        let view = state.view().expect("open");
        let most = view.placed.most;
        assert_eq!(view.placed.scroll, 0, "back at the top");
        state.update(Event::Act(back, Act::Focus));
        assert_eq!(state.view().expect("open").placed.scroll, most);
    }

    #[test]
    fn the_wheel_off_a_slider_scrolls_and_never_past_the_ends() {
        let mut state = cramped();
        let at = on(&state, row("speakers"), 0.5);
        state.update(Event::Motion(at));
        assert!(state.update(Event::Wheel(-1)).is_empty(), "at the top");
        assert_eq!(state.update(Event::Wheel(1)), [Effect::Redraw]);
        assert_eq!(state.view().expect("open").placed.scroll, NOTCH);
        state.update(Event::Wheel(100));
        let view = state.view().expect("open");
        assert_eq!(view.placed.scroll, view.placed.most, "the bottom");
        state.update(Event::Motion((5.0, 5.0)));
        assert!(
            state.update(Event::Wheel(-1)).is_empty(),
            "off the mixer, nothing"
        );
    }

    #[test]
    fn nothing_closed_answers_nothing() {
        let mut state = State::default();
        assert!(state.update(Event::Key(Key::Escape)).is_empty());
        assert!(state.update(Event::Wheel(1)).is_empty());
        assert_eq!(state.open_on(), None);
    }
}
