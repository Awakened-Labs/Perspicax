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
//! server says it is there; the wheel over a slider steps it. A press off
//! the mixer closes it, as do Escape and losing the keyboard.
//!
//! The keyboard moves through the controls with Tab and Shift+Tab, or the
//! arrows up and down. On a slider, left and right step it, Page Up and
//! Page Down move it ten at a time, and Home and End take it to silence or
//! to the limit; on a mute button, Space or Return presses it. The control
//! the keyboard is on is ringed once a key is used, and not while the
//! pointer is what is used.

use perspicax_config::Edge;

use crate::{
    layout::{
        Rect,
        mixer::{Control, Mixed, Placed, lay_out, percent_at},
    },
    model::{
        Button,
        sound::{Change, Side, stepped},
    },
};

/// How far Page Up and Page Down move a slider, in percent.
const PAGE: u32 = 10;

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
    /// The mixer was asked for on the monitor named `output`, from its
    /// panel item at `button`, on a panel along `edge`, with `area` the
    /// room beside the panel: open it, showing `mixed`.
    Open {
        output: String,
        area: Rect,
        button: Rect,
        edge: Edge,
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
}

/// The mixer, if it is open.
#[derive(Debug, Default)]
pub(crate) struct State {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    output: String,
    placed: Placed,
    mixed: Mixed,
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
                area,
                button,
                edge,
                mixed,
            } => {
                self.open = Some(Open {
                    output,
                    placed: lay_out(button, edge, area),
                    mixed,
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
                self.keyed = false;
                match self.placed.at(at) {
                    Some(Control::Mute(side)) => {
                        self.focus = Control::Mute(side);
                        self.mute(side)
                    }
                    Some(Control::Volume(side)) => {
                        self.focus = Control::Volume(side);
                        self.dragging = Some(side);
                        let mut effects = self.set_at(side, at.0);
                        effects.push(Effect::Redraw);
                        effects
                    }
                    None => Vec::new(),
                }
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
            Event::Wheel(notches) => {
                let control = self.pointer.and_then(|at| self.placed.at(at));
                match control {
                    Some(Control::Volume(side) | Control::Mute(side)) => self.step(side, -notches),
                    None => Vec::new(),
                }
            }
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

    /// Mute `side`'s device, if it has one.
    fn mute(&self, side: Side) -> Vec<Effect> {
        match self.mixed.on(side) {
            Some(_) => vec![Effect::Mute(side)],
            None => Vec::new(),
        }
    }

    /// The control `by` places on from the focused one, round the ends.
    fn move_focus(&mut self, by: isize) -> Vec<Effect> {
        let all = Control::ALL;
        let at = all
            .iter()
            .position(|&control| control == self.focus)
            .unwrap_or(0);
        let next = (at as isize + by).rem_euclid(all.len() as isize) as usize;
        self.focus = all[next];
        vec![Effect::Redraw]
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        let slider = match self.focus {
            Control::Volume(side) => Some(side),
            Control::Mute(_) => None,
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
            (Key::Press, None) => match self.focus {
                Control::Mute(side) => self.mute(side),
                Control::Volume(_) => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn act(&mut self, control: Control, act: Act) -> Vec<Effect> {
        match (control, act) {
            (Control::Mute(side), Act::Click) => self.mute(side),
            (Control::Volume(side), Act::Increment) => self.step(side, 1),
            (Control::Volume(side), Act::Decrement) => self.step(side, -1),
            (Control::Volume(side), Act::Set(percent)) => self.set(side, percent),
            (control, Act::Focus) => {
                self.focus = control;
                vec![Effect::Redraw]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{layout::mixer::Master, model::sound::Level};

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
        }
    }

    fn opened(mixed: Mixed) -> State {
        let mut state = State::default();
        let effects = state.update(Event::Open {
            output: "DP-1".to_owned(),
            area: Rect::new(0, 0, 1280, 760),
            button: Rect::new(1180, 760, 100, 40),
            edge: Edge::Bottom,
            mixed,
        });
        assert_eq!(effects, [Effect::Redraw]);
        state
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
    fn nothing_closed_answers_nothing() {
        let mut state = State::default();
        assert!(state.update(Event::Key(Key::Escape)).is_empty());
        assert!(state.update(Event::Wheel(1)).is_empty());
        assert_eq!(state.open_on(), None);
    }
}
