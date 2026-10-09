//! What a pie does when something happens: perspicax asking for one, the
//! pointer, the buttons, the wheel, a key.
//!
//! A reducer, as [`crate::update`] is for the menus: an [`Event`] and the
//! [`State`] in, the state changed and the [`Effect`]s to carry out back,
//! tested as values.
//!
//! One pie is open at a time. Asking for the one that is open closes it,
//! and asking for another puts it in its place. Pointing at a slot picks
//! it, by direction (see [`crate::layout::pie`]); a button acts on what is
//! picked when it comes up, and only if it went down on the pie. The left
//! button opens a submenu in the pie's place, or starts a program, or,
//! when a program has windows open, raises the next of them; the middle
//! one starts the program whatever is open; the right one goes back out of
//! a submenu, and closes the pie from its first ring. The wheel spins the
//! pie a place a notch. Escape closes it, Enter is the left button, the
//! arrows up and down spin it, and Backspace is the right button. Losing
//! the keyboard closes it.

use crate::{
    layout::{Rect, pie::Ring},
    model::{
        Button,
        apps::Run,
        pie::{Does, Slot},
    },
};

/// A key pressed while a pie has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Escape,
    Enter,
    Up,
    Down,
    Back,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    /// perspicax asked for the pie `name`, `at` a point on a monitor whose
    /// room for it is `area`.
    Asked {
        name: String,
        output: String,
        area: Rect,
        size: u32,
        at: (i32, i32),
        slots: Vec<Slot>,
    },
    /// The pointer came onto the pie's surface, or moved on it.
    Motion((f64, f64)),
    /// It left the surface.
    Leave,
    Press((f64, f64), Button),
    Release((f64, f64), Button),
    /// The wheel turned this many notches, up negative.
    Wheel(i32),
    Key(Key),
    /// The pie's surface lost the keyboard to something else.
    KeyboardLost,
}

/// What to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// What the pie shows changed: draw it again, or take it away.
    Redraw,
    /// Start a program.
    Run(Run),
}

/// The pie that is open, if one is.
#[derive(Debug, Default)]
pub(crate) struct State {
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    name: String,
    output: String,
    area: Rect,
    size: u32,
    /// Where it opened.
    at: (i32, i32),
    /// The first ring, then each submenu opened from the one before.
    levels: Vec<Level>,
    /// Where the pointer is on the surface, once it is known.
    pointer: Option<(f64, f64)>,
    /// The button that went down on the pie and has not come up.
    pressed: Option<Button>,
}

#[derive(Debug)]
struct Level {
    /// What it is called: the pie's name, or its submenu's label.
    label: String,
    slots: Vec<Slot>,
    spin: i32,
}

/// What an open pie shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct View<'a> {
    pub(crate) output: &'a str,
    /// The pie's name, or the label of the submenu open.
    pub(crate) label: &'a str,
    pub(crate) ring: Ring,
    pub(crate) slots: &'a [Slot],
    pub(crate) spin: i32,
    pub(crate) pointer: Option<(f64, f64)>,
    /// The slot pointed at.
    pub(crate) picked: Option<usize>,
}

impl State {
    /// What is shown, while a pie is open.
    pub(crate) fn view(&self) -> Option<View<'_>> {
        let open = self.open.as_ref()?;
        let level = open.levels.last()?;
        let ring = open.ring();
        Some(View {
            output: &open.output,
            label: &level.label,
            ring,
            slots: &level.slots,
            spin: level.spin,
            pointer: open.pointer,
            picked: open.picked(),
        })
    }

    /// Handle `event`.
    pub(crate) fn update(&mut self, event: Event) -> Vec<Effect> {
        if let Event::Asked {
            name,
            output,
            area,
            size,
            at,
            slots,
        } = event
        {
            let same = self
                .open
                .as_ref()
                .is_some_and(|open| open.name == name && open.output == output);
            self.open = (!same).then(|| Open {
                levels: vec![Level {
                    label: name.clone(),
                    slots,
                    spin: 0,
                }],
                name,
                output,
                area,
                size,
                at,
                pointer: None,
                pressed: None,
            });
            return vec![Effect::Redraw];
        }
        let Some(open) = self.open.as_mut() else {
            return Vec::new();
        };
        let before = (open.picked(), open.levels.len());
        let (then, mut effects) = match event {
            Event::Asked { .. } => unreachable!("handled above"),
            Event::Motion(at) => {
                open.pointer = Some(at);
                (Then::Stay, Vec::new())
            }
            Event::Leave => {
                open.pointer = None;
                (Then::Stay, Vec::new())
            }
            Event::Press(at, button) => {
                open.pointer = Some(at);
                open.pressed = Some(button);
                (Then::Stay, Vec::new())
            }
            Event::Release(at, button) => {
                open.pointer = Some(at);
                if open.pressed.take() == Some(button) {
                    match button {
                        Button::Left => open.choose(open.picked()),
                        Button::Middle => open.launch(open.picked()),
                        Button::Right => open.back(),
                        Button::Other => (Then::Stay, Vec::new()),
                    }
                } else {
                    (Then::Stay, Vec::new())
                }
            }
            Event::Wheel(notches) => open.spin(-notches),
            Event::Key(Key::Escape) | Event::KeyboardLost => (Then::Close, Vec::new()),
            Event::Key(Key::Enter) => open.choose(open.picked()),
            Event::Key(Key::Up) => open.spin(1),
            Event::Key(Key::Down) => open.spin(-1),
            Event::Key(Key::Back) => open.back(),
        };
        match then {
            Then::Close => self.open = None,
            Then::Stay if (open.picked(), open.levels.len()) == before => {}
            Then::Stay => effects.push(Effect::Redraw),
        }
        if matches!(then, Then::Close) {
            effects.push(Effect::Redraw);
        }
        effects.dedup();
        effects
    }
}

/// Whether the pie stays open after an event.
enum Then {
    Stay,
    Close,
}

impl Open {
    fn ring(&self) -> Ring {
        let places = self.levels.last().map_or(0, |level| level.slots.len());
        Ring::new(places, self.size, self.at, self.area)
    }

    /// The slot pointed at.
    fn picked(&self) -> Option<usize> {
        let level = self.levels.last()?;
        self.ring().pointed(self.pointer?, level.spin)
    }

    /// Spin the ring open `by` places, clockwise.
    fn spin(&mut self, by: i32) -> (Then, Vec<Effect>) {
        if let Some(level) = self.levels.last_mut() {
            level.spin += by;
        }
        (Then::Stay, vec![Effect::Redraw])
    }

    /// Do what slot `index` does: open its submenu, or start its program.
    fn choose(&mut self, index: Option<usize>) -> (Then, Vec<Effect>) {
        let Some(slot) = index.and_then(|index| self.levels.last()?.slots.get(index)) else {
            return (Then::Stay, Vec::new());
        };
        match &slot.does {
            Does::Open(below) => {
                let level = Level {
                    label: slot.label.clone(),
                    slots: below.clone(),
                    spin: 0,
                };
                self.levels.push(level);
                (Then::Stay, Vec::new())
            }
            Does::Launch(run) => (Then::Close, vec![Effect::Run(run.clone())]),
        }
    }

    /// Start slot `index`'s program, whatever it has open.
    fn launch(&mut self, index: Option<usize>) -> (Then, Vec<Effect>) {
        match index
            .and_then(|index| self.levels.last()?.slots.get(index))
            .map(|slot| &slot.does)
        {
            Some(Does::Launch(run)) => (Then::Close, vec![Effect::Run(run.clone())]),
            _ => (Then::Stay, Vec::new()),
        }
    }

    /// Out of the submenu open, or closed from the first ring.
    fn back(&mut self) -> (Then, Vec<Effect>) {
        if self.levels.len() > 1 {
            self.levels.pop();
            (Then::Stay, Vec::new())
        } else {
            (Then::Close, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1048);
    const MIDDLE: (i32, i32) = (960, 500);
    /// Straight up from the middle, and straight right, out at the edges.
    const UP: (f64, f64) = (960.0, 2.0);
    const RIGHT: (f64, f64) = (1918.0, 500.0);
    const DOWN: (f64, f64) = (960.0, 1046.0);

    fn run(program: &str) -> Run {
        Run {
            argv: vec![program.to_owned()],
            terminal: false,
            dir: None,
        }
    }

    fn launcher(label: &str) -> Slot {
        Slot {
            label: label.to_owned(),
            icons: Vec::new(),
            does: Does::Launch(run(label)),
        }
    }

    fn submenu(label: &str, slots: Vec<Slot>) -> Slot {
        Slot {
            label: label.to_owned(),
            icons: Vec::new(),
            does: Does::Open(slots),
        }
    }

    /// A pie of a terminal (up), a submenu of games (right), firefox
    /// (down) and an editor (left), open in the middle of the screen.
    fn opened() -> State {
        let mut state = State::default();
        state.update(asked("launchers"));
        state
    }

    fn asked(name: &str) -> Event {
        Event::Asked {
            name: name.to_owned(),
            output: "eDP-1".to_owned(),
            area: SCREEN,
            size: 512,
            at: MIDDLE,
            slots: vec![
                launcher("terminal"),
                submenu("games", vec![launcher("steam"), launcher("pyfa")]),
                launcher("firefox"),
                launcher("editor"),
            ],
        }
    }

    fn click(state: &mut State, at: (f64, f64), button: Button) -> Vec<Effect> {
        state.update(Event::Motion(at));
        state.update(Event::Press(at, button));
        state.update(Event::Release(at, button))
    }

    fn picked(state: &State) -> Option<&str> {
        let view = state.view()?;
        view.picked.map(|index| view.slots[index].label.as_str())
    }

    #[test]
    fn asking_opens_the_pie_and_asking_again_closes_it() {
        let mut state = State::default();
        assert_eq!(state.update(asked("launchers")), [Effect::Redraw]);
        let view = state.view().expect("open");
        assert_eq!(
            (view.output, view.label, view.slots.len()),
            ("eDP-1", "launchers", 4)
        );
        assert_eq!(view.ring.centre, (960.0, 500.0));
        assert_eq!(state.update(asked("launchers")), [Effect::Redraw]);
        assert!(state.view().is_none());
        // Another pie takes the open one's place.
        state.update(asked("launchers"));
        state.update(asked("windows"));
        assert_eq!(state.view().expect("open").label, "windows");
    }

    #[test]
    fn pointing_picks_by_direction_and_only_a_change_is_drawn() {
        let mut state = opened();
        assert_eq!(picked(&state), None, "the pointer is not known yet");
        assert_eq!(state.update(Event::Motion(UP)), [Effect::Redraw]);
        assert_eq!(picked(&state), Some("terminal"));
        assert_eq!(state.update(Event::Motion((961.0, 3.0))), []);
        state.update(Event::Motion(RIGHT));
        assert_eq!(picked(&state), Some("games"));
        state.update(Event::Motion((960.0, 500.0)));
        assert_eq!(picked(&state), None, "the dead middle");
    }

    #[test]
    fn a_left_click_starts_a_program_and_closes_the_pie() {
        let mut state = opened();
        assert_eq!(
            click(&mut state, UP, Button::Left),
            [Effect::Run(run("terminal")), Effect::Redraw]
        );
        assert!(state.view().is_none());
    }

    #[test]
    fn a_left_click_on_a_submenu_opens_it_and_a_right_click_goes_back_then_closes() {
        let mut state = opened();
        assert_eq!(click(&mut state, RIGHT, Button::Left), [Effect::Redraw]);
        let view = state.view().expect("open");
        assert_eq!((view.label, view.slots.len()), ("games", 2));
        // In the submenu up is steam; down is pyfa.
        state.update(Event::Motion(DOWN));
        assert_eq!(picked(&state), Some("pyfa"));
        assert_eq!(click(&mut state, DOWN, Button::Right), [Effect::Redraw]);
        assert_eq!(state.view().expect("open").label, "launchers");
        assert_eq!(click(&mut state, DOWN, Button::Right), [Effect::Redraw]);
        assert!(
            state.view().is_none(),
            "the right button closes the first ring"
        );
    }

    #[test]
    fn the_middle_button_starts_a_program_and_does_nothing_to_a_submenu() {
        let mut state = opened();
        assert_eq!(click(&mut state, RIGHT, Button::Middle), []);
        assert!(state.view().is_some());
        assert_eq!(
            click(&mut state, DOWN, Button::Middle),
            [Effect::Run(run("firefox")), Effect::Redraw]
        );
    }

    #[test]
    fn a_release_whose_press_was_not_on_the_pie_does_nothing() {
        let mut state = opened();
        state.update(Event::Motion(UP));
        assert_eq!(state.update(Event::Release(UP, Button::Left)), []);
        state.update(Event::Press(UP, Button::Right));
        assert_eq!(state.update(Event::Release(UP, Button::Left)), []);
        assert!(state.view().is_some());
    }

    #[test]
    fn the_wheel_and_the_arrows_spin_the_pie_and_what_is_picked_with_it() {
        let mut state = opened();
        state.update(Event::Motion(UP));
        assert_eq!(state.update(Event::Wheel(-1)), [Effect::Redraw]);
        assert_eq!(state.view().expect("open").spin, 1);
        assert_eq!(
            picked(&state),
            Some("editor"),
            "the last spun up to the top"
        );
        state.update(Event::Key(Key::Down));
        state.update(Event::Key(Key::Down));
        assert_eq!(picked(&state), Some("games"));
        state.update(Event::Wheel(1));
        state.update(Event::Key(Key::Up));
        assert_eq!(picked(&state), Some("games"));
    }

    #[test]
    fn keys_choose_go_back_and_close() {
        let mut state = opened();
        state.update(Event::Motion(RIGHT));
        assert_eq!(state.update(Event::Key(Key::Enter)), [Effect::Redraw]);
        assert_eq!(state.view().expect("open").label, "games");
        state.update(Event::Key(Key::Back));
        assert_eq!(state.view().expect("open").label, "launchers");
        assert_eq!(state.update(Event::Key(Key::Escape)), [Effect::Redraw]);
        assert!(state.view().is_none());
        let mut state = opened();
        state.update(Event::KeyboardLost);
        assert!(state.view().is_none());
    }
}
