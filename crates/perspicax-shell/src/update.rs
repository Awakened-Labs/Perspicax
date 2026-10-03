//! What the shell does when something happens: a right-click on the
//! desktop, a key in a menu, perspicax asking for the root menu.
//!
//! A reducer: an [`Event`] and the [`State`] in, the state changed and the
//! [`Effect`]s to carry out back. Nothing here draws or speaks Wayland, so
//! the shell's behaviour is tested here, as values.
//!
//! A menu works the way menus do on any desktop. The pointer selects the
//! item under it, and a submenu opens as soon as the pointer is on its item;
//! a click chooses. The arrows move up and down a menu, into a submenu and
//! back out; Enter chooses and Escape closes. Typing narrows the menu to
//! the programs whose names hold what was typed, from anywhere in it, as
//! many of the best as fit in one column, and Backspace widens it again. A click anywhere off the menus closes them,
//! and so does losing the keyboard to something else.

use crate::{
    layout::{
        Measure, Rect,
        menu::{self, Line, Placed, Shown},
    },
    model::{
        apps::Run,
        menu::{Does, Item, Menu, Route},
    },
};

/// A pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
    Other,
}

/// A key pressed while a menu has the keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
    Backspace,
    /// A key that types: its text.
    Text(String),
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    /// A button went down on a monitor's desktop, `at` a point on it.
    #[cfg_attr(
        not(feature = "wallpaper"),
        allow(
            dead_code,
            reason = "a shell without wallpapers has no desktop to click"
        )
    )]
    DesktopPress {
        output: String,
        area: Rect,
        at: (f64, f64),
        button: Button,
    },
    /// perspicax asked for the root menu `at` a point on a monitor: a key
    /// binding, with the pointer there.
    RootMenu {
        output: String,
        area: Rect,
        at: (i32, i32),
    },
    /// The pointer moved on the menus.
    Motion((f64, f64)),
    /// A button went down on the menus' surface: on a menu, or off them.
    Press((f64, f64)),
    /// A button came up on the menus' surface.
    Release((f64, f64)),
    Key(Key),
    /// The menus' surface lost the keyboard to something else.
    KeyboardLost,
    /// The compositor gave the menus' surface this size.
    Resized(Rect),
    /// An assistive technology chose the item at this route.
    Choose(Route),
    /// An assistive technology moved to the item at this route.
    Select(Route),
}

/// What to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// What the menus show changed: draw them again, or take them away.
    Redraw,
    /// Start a program.
    Run(Run),
    /// Ask perspicax to end the session.
    LogOut,
}

/// What the shell's menus are doing.
#[derive(Debug)]
pub(crate) struct State {
    root: Menu,
    open: Option<Open>,
}

/// The menus while they are open.
#[derive(Debug)]
struct Open {
    /// The monitor they are on, by connector name.
    output: String,
    /// The monitor, in its own logical pixels.
    area: Rect,
    /// Where they were asked for.
    anchor: (i32, i32),
    /// What has been typed.
    query: String,
    /// The root menu, or what typing narrowed it to; then each open submenu.
    levels: Vec<Level>,
    /// The levels, laid out.
    placed: Vec<Placed>,
    /// A button went down on a menu and has not come up.
    pressed: bool,
}

/// One open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Level {
    /// The route of each of its lines.
    lines: Vec<Route>,
    /// The line selected.
    selected: Option<usize>,
}

/// How a line comes to be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// The pointer is on it: a submenu opens, nothing in it selected.
    Pointed,
    /// The arrows moved to it: a submenu stays shut.
    Moved,
    /// It was entered: a submenu opens with its first item selected.
    Entered,
}

/// The open menus, for drawing and for the accessibility tree.
#[derive(Debug)]
pub(crate) struct View<'a> {
    pub(crate) output: &'a str,
    /// What has been typed, if anything.
    pub(crate) query: Option<&'a str>,
    pub(crate) menus: Vec<MenuView<'a>>,
}

/// One open menu.
#[derive(Debug)]
pub(crate) struct MenuView<'a> {
    pub(crate) rect: Rect,
    /// Where what has been typed is shown.
    pub(crate) header: Option<Rect>,
    /// The item that opened it; `None` for the first.
    pub(crate) opened_by: Option<&'a Item>,
    pub(crate) lines: Vec<LineView<'a>>,
}

/// One line of an open menu.
#[derive(Debug)]
pub(crate) struct LineView<'a> {
    pub(crate) rect: Rect,
    pub(crate) route: &'a [usize],
    pub(crate) item: &'a Item,
    /// Selected: drawn highlighted.
    pub(crate) selected: bool,
    /// Its submenu is open beside it.
    pub(crate) open: bool,
    /// Where the keyboard is.
    pub(crate) focused: bool,
}

impl State {
    pub(crate) fn new(root: Menu) -> Self {
        Self { root, open: None }
    }

    /// Show `root` from the next time the menu opens. Ignored while it is
    /// open, so that what a person is looking at does not shift under them.
    pub(crate) fn set_root(&mut self, root: Menu) {
        if self.open.is_none() {
            self.root = root;
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Handle `event`, measuring text with `measure`.
    pub(crate) fn update(&mut self, event: Event, measure: &mut impl Measure) -> Vec<Effect> {
        let effects = match event {
            Event::DesktopPress {
                output,
                area,
                at,
                button,
            } => match button {
                Button::Right => {
                    self.open(output, area, (at.0.round() as i32, at.1.round() as i32))
                }
                _ => self.close(),
            },
            Event::RootMenu { output, area, at } => {
                if self.open.is_some() {
                    self.close()
                } else {
                    self.open(output, area, at)
                }
            }
            Event::Motion(at) => self.hover(at),
            Event::Press(at) => self.press(at),
            Event::Release(at) => self.release(at),
            Event::Key(key) => self.key(key),
            Event::KeyboardLost => self.close(),
            Event::Resized(area) => match &mut self.open {
                Some(open) if open.area != area => {
                    open.area = area;
                    vec![Effect::Redraw]
                }
                _ => Vec::new(),
            },
            Event::Choose(route) => match self.find(&route) {
                Some((level, line)) => self.activate(level, line, How::Entered),
                None => Vec::new(),
            },
            Event::Select(route) => match self.find(&route) {
                Some((level, line)) => self.select(level, line, How::Moved),
                None => Vec::new(),
            },
        };
        if effects.contains(&Effect::Redraw) {
            self.lay_out(measure);
        }
        effects
    }

    /// The open menus, if they are open.
    pub(crate) fn view(&self) -> Option<View<'_>> {
        let open = self.open.as_ref()?;
        let active = open.active();
        let menus = open
            .levels
            .iter()
            .zip(&open.placed)
            .enumerate()
            .map(|(k, (level, placed))| MenuView {
                rect: placed.rect,
                header: placed.header,
                opened_by: k
                    .checked_sub(1)
                    .and_then(|above| open.levels[above].selected_route())
                    .and_then(|route| self.root.item(route)),
                lines: level
                    .lines
                    .iter()
                    .zip(&placed.rows)
                    .enumerate()
                    .filter_map(|(n, (route, rect))| {
                        let selected = level.selected == Some(n);
                        Some(LineView {
                            rect: *rect,
                            route,
                            item: self.root.item(route)?,
                            selected,
                            open: selected && open.levels.len() > k + 1,
                            focused: selected && k == active,
                        })
                    })
                    .collect(),
            })
            .collect();
        Some(View {
            output: &open.output,
            query: (!open.query.is_empty()).then_some(open.query.as_str()),
            menus,
        })
    }

    fn open(&mut self, output: String, area: Rect, anchor: (i32, i32)) -> Vec<Effect> {
        if self.root.items.is_empty() {
            tracing::info!("the root menu has nothing in it to show");
            return self.close();
        }
        self.open = Some(Open {
            output,
            area,
            anchor,
            query: String::new(),
            levels: vec![Level::of(&self.root, &[])],
            placed: Vec::new(),
            pressed: false,
        });
        vec![Effect::Redraw]
    }

    fn close(&mut self) -> Vec<Effect> {
        match self.open.take() {
            Some(_) => vec![Effect::Redraw],
            None => Vec::new(),
        }
    }

    fn hover(&mut self, at: (f64, f64)) -> Vec<Effect> {
        match self
            .open
            .as_ref()
            .and_then(|open| menu::line_at(&open.placed, at))
        {
            Some((level, line)) => self.select(level, line, How::Pointed),
            None => Vec::new(),
        }
    }

    fn press(&mut self, at: (f64, f64)) -> Vec<Effect> {
        let Some(open) = &mut self.open else {
            return Vec::new();
        };
        if menu::on_menus(&open.placed, at) {
            open.pressed = true;
            Vec::new()
        } else {
            self.close()
        }
    }

    fn release(&mut self, at: (f64, f64)) -> Vec<Effect> {
        let Some(open) = &mut self.open else {
            return Vec::new();
        };
        if !std::mem::take(&mut open.pressed) {
            return Vec::new();
        }
        match menu::line_at(&open.placed, at) {
            Some((level, line)) => self.activate(level, line, How::Pointed),
            None => Vec::new(),
        }
    }

    fn key(&mut self, key: Key) -> Vec<Effect> {
        let Some(open) = &self.open else {
            return Vec::new();
        };
        let active = open.active();
        let level = &open.levels[active];
        let choosable: Vec<usize> = (0..level.lines.len())
            .filter(|&n| self.root.item(&level.lines[n]).is_some_and(Item::choosable))
            .collect();
        let count = choosable.len();
        let at = level
            .selected
            .and_then(|selected| choosable.iter().position(|&n| n == selected));
        let selected = level.selected;
        let into = selected.filter(|&line| {
            self.root
                .item(&level.lines[line])
                .and_then(Item::submenu)
                .is_some()
        });

        let step = match (&key, count) {
            (_, 0) => None,
            (Key::Down, _) => Some(at.map_or(0, |at| (at + 1) % count)),
            (Key::Up, _) => Some(at.map_or(count - 1, |at| (at + count - 1) % count)),
            (Key::Home, _) => Some(0),
            (Key::End, _) => Some(count - 1),
            _ => None,
        };
        if let Some(step) = step {
            return self.select(active, choosable[step], How::Moved);
        }
        match key {
            Key::Right => match into {
                Some(line) => self.select(active, line, How::Entered),
                None => Vec::new(),
            },
            Key::Left if active > 0 => {
                if let Some(open) = &mut self.open {
                    open.levels.truncate(active);
                }
                vec![Effect::Redraw]
            }
            Key::Enter => match selected {
                Some(line) => self.activate(active, line, How::Entered),
                None => Vec::new(),
            },
            Key::Escape => self.close(),
            Key::Backspace => self.typed(|query| query.pop().is_some()),
            Key::Text(text) => self.typed(|query| {
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                // A space first is no search, and nothing to show for one.
                if text.is_empty() || (query.is_empty() && text.trim().is_empty()) {
                    return false;
                }
                query.push_str(&text);
                true
            }),
            Key::Left | Key::Down | Key::Up | Key::Home | Key::End => Vec::new(),
        }
    }

    /// Change what has been typed with `edit`, which says whether it did,
    /// and show what it finds now.
    fn typed(&mut self, edit: impl FnOnce(&mut String) -> bool) -> Vec<Effect> {
        let edited = self.open.as_mut().is_some_and(|open| edit(&mut open.query));
        if edited { self.narrow() } else { Vec::new() }
    }

    /// Show what the query finds, or the whole root menu for none.
    fn narrow(&mut self) -> Vec<Effect> {
        let Some(open) = &mut self.open else {
            return Vec::new();
        };
        open.levels = if open.query.is_empty() {
            vec![Level::of(&self.root, &[])]
        } else {
            let mut found = self.root.find(&open.query);
            found.truncate(menu::found_fit(open.area));
            vec![Level {
                selected: (!found.is_empty()).then_some(0),
                lines: found,
            }]
        };
        vec![Effect::Redraw]
    }

    /// Select line `line` of open menu `level`, closing what was open
    /// below it, and opening its submenu as `how` says.
    fn select(&mut self, level: usize, line: usize, how: How) -> Vec<Effect> {
        let Some(open) = &mut self.open else {
            return Vec::new();
        };
        let Some(route) = open.levels.get(level).and_then(|it| it.lines.get(line)) else {
            return Vec::new();
        };
        let Some(item) = self.root.item(route).filter(|item| item.choosable()) else {
            return Vec::new();
        };
        let before = open.levels.clone();
        let route = route.clone();
        open.levels.truncate(level + 1);
        open.levels[level].selected = Some(line);
        if let Some(submenu) = item.submenu() {
            match how {
                How::Moved => {}
                How::Pointed
                    if before.len() > level + 1 && before[level].selected == Some(line) =>
                {
                    // Already open, perhaps with a line in it selected.
                    open.levels.push(before[level + 1].clone());
                }
                How::Pointed => open.levels.push(Level::of(&self.root, &route)),
                How::Entered => {
                    let mut below = Level::of(&self.root, &route);
                    below.selected = submenu.items.iter().position(Item::choosable);
                    open.levels.push(below);
                }
            }
        }
        if open.levels == before {
            Vec::new()
        } else {
            vec![Effect::Redraw]
        }
    }

    /// Choose line `line` of open menu `level`: start its program and close
    /// the menus, or open its submenu.
    fn activate(&mut self, level: usize, line: usize, how: How) -> Vec<Effect> {
        let Some(open) = &self.open else {
            return Vec::new();
        };
        let Some(item) = open
            .levels
            .get(level)
            .and_then(|it| it.lines.get(line))
            .and_then(|route| self.root.item(route))
        else {
            return Vec::new();
        };
        match &item.does {
            Does::Run(run) => {
                let run = run.clone();
                let mut effects = self.close();
                effects.push(Effect::Run(run));
                effects
            }
            Does::LogOut => {
                let mut effects = self.close();
                effects.push(Effect::LogOut);
                effects
            }
            Does::Open(_) => self.select(level, line, how),
            Does::Separator => Vec::new(),
        }
    }

    /// The open menu and line showing the item at `route`.
    fn find(&self, route: &[usize]) -> Option<(usize, usize)> {
        self.open
            .as_ref()?
            .levels
            .iter()
            .enumerate()
            .find_map(|(level, it)| Some((level, it.lines.iter().position(|line| line == route)?)))
    }

    /// Lay the open menus out again.
    fn lay_out(&mut self, measure: &mut impl Measure) {
        let Self { root, open } = self;
        let Some(open) = open else {
            return;
        };
        let shown: Vec<Shown<'_>> = open
            .levels
            .iter()
            .enumerate()
            .map(|(k, level)| Shown {
                header: (k == 0 && !open.query.is_empty()).then_some(open.query.as_str()),
                lines: level
                    .lines
                    .iter()
                    .map(|route| match root.item(route) {
                        Some(item) if item.choosable() => Line::Item {
                            label: &item.label,
                            submenu: item.submenu().is_some(),
                        },
                        _ => Line::Separator,
                    })
                    .collect(),
                from: k
                    .checked_sub(1)
                    .and_then(|above| open.levels[above].selected),
            })
            .collect();
        open.placed = menu::cascade(&shown, open.anchor, open.area, measure);
    }
}

impl Open {
    /// The menu the keyboard is in: the deepest with a line selected.
    fn active(&self) -> usize {
        self.levels
            .iter()
            .rposition(|level| level.selected.is_some())
            .unwrap_or(0)
    }
}

impl Level {
    /// The menu at `route`, nothing selected.
    fn of(root: &Menu, route: &[usize]) -> Self {
        let count = root.menu(route).map_or(0, |menu| menu.items.len());
        Self {
            lines: (0..count)
                .map(|at| {
                    let mut line = route.to_vec();
                    line.push(at);
                    line
                })
                .collect(),
            selected: None,
        }
    }

    fn selected_route(&self) -> Option<&Route> {
        self.lines.get(self.selected?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        layout::Monospace,
        model::{
            apps::App,
            menu::{Session, root},
        },
    };

    const SCREEN: Rect = Rect::new(0, 0, 1280, 800);

    fn app(id: &str, name: &str, categories: &str) -> App {
        App {
            id: id.to_owned(),
            name: name.to_owned(),
            comment: None,
            icon: None,
            run: run(id),
            categories: vec![categories.to_owned()],
            keywords: Vec::new(),
            wm_class: None,
        }
    }

    fn run(program: &str) -> Run {
        Run {
            argv: vec![program.to_owned()],
            terminal: false,
            dir: None,
        }
    }

    /// The root menu: Accessories (gedit, xcalc), Internet (firefox), a
    /// separator, Lock, Log Out.
    fn state() -> State {
        State::new(root(
            &[
                app("gedit", "Text Editor", "Utility"),
                app("xcalc", "Calculator", "Utility"),
                app("firefox", "Firefox", "Network"),
            ],
            None,
            &Session {
                lock: Some(run("swaylock")),
                log_out: true,
            },
        ))
    }

    struct Shell {
        state: State,
    }

    impl Shell {
        fn new() -> Self {
            Self { state: state() }
        }

        fn send(&mut self, event: Event) -> Vec<Effect> {
            self.state.update(event, &mut Monospace(8.0))
        }

        fn right_click(&mut self, at: (f64, f64)) -> Vec<Effect> {
            self.send(Event::DesktopPress {
                output: "DP-1".to_owned(),
                area: SCREEN,
                at,
                button: Button::Right,
            })
        }

        fn key(&mut self, key: Key) -> Vec<Effect> {
            self.send(Event::Key(key))
        }

        /// Each open menu's labels, `*` before the selected one.
        fn shown(&self) -> Vec<Vec<String>> {
            self.state
                .view()
                .map(|view| {
                    view.menus
                        .iter()
                        .map(|menu| {
                            menu.lines
                                .iter()
                                .map(|line| match (&line.item.does, line.selected) {
                                    (Does::Separator, _) => "--".to_owned(),
                                    (_, true) => format!("*{}", line.item.label),
                                    (_, false) => line.item.label.clone(),
                                })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default()
        }

        /// The middle of the line labelled `label`.
        fn middle_of(&self, label: &str) -> (f64, f64) {
            let view = self.state.view().expect("open");
            let rect = view
                .menus
                .iter()
                .flat_map(|menu| &menu.lines)
                .find(|line| line.item.label == label)
                .expect("shown")
                .rect;
            (
                f64::from(rect.x + rect.w / 2),
                f64::from(rect.y + rect.h / 2),
            )
        }

        /// Point at `at`, press and release there, as a click does; what
        /// the release did.
        fn click(&mut self, at: (f64, f64)) -> Vec<Effect> {
            self.send(Event::Motion(at));
            self.send(Event::Press(at));
            self.send(Event::Release(at))
        }
    }

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn a_right_click_on_the_desktop_opens_the_root_menu_at_the_pointer() {
        let mut shell = Shell::new();
        assert_eq!(shell.right_click((300.4, 200.6)), [Effect::Redraw]);
        let view = shell.state.view().expect("open");
        assert_eq!(view.output, "DP-1");
        assert_eq!((view.menus[0].rect.x, view.menus[0].rect.y), (300, 201));
        assert_eq!(
            shell.shown(),
            [rows(&["Accessories", "Internet", "--", "Lock", "Log Out"])]
        );

        let mut left = Shell::new();
        left.send(Event::DesktopPress {
            output: "DP-1".to_owned(),
            area: SCREEN,
            at: (300.0, 200.0),
            button: Button::Left,
        });
        assert!(!left.state.is_open(), "a left click opens nothing");
    }

    #[test]
    fn the_root_menu_action_opens_it_and_a_second_closes_it() {
        let mut shell = Shell::new();
        let ask = Event::RootMenu {
            output: "DP-1".to_owned(),
            area: SCREEN,
            at: (640, 400),
        };
        assert_eq!(shell.send(ask.clone()), [Effect::Redraw]);
        assert_eq!(
            shell
                .state
                .view()
                .map(|view| (view.menus[0].rect.x, view.menus[0].rect.y)),
            Some((640, 400))
        );
        assert_eq!(shell.send(ask), [Effect::Redraw]);
        assert!(!shell.state.is_open());
    }

    #[test]
    fn pointing_at_a_group_opens_it_beside_the_menu() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.send(Event::Motion(shell.middle_of("Accessories")));
        assert_eq!(
            shell.shown(),
            [
                rows(&["*Accessories", "Internet", "--", "Lock", "Log Out"]),
                rows(&["Calculator", "Text Editor"]),
            ]
        );
        shell.send(Event::Motion(shell.middle_of("Calculator")));
        shell.send(Event::Motion(shell.middle_of("Accessories")));
        assert_eq!(
            shell.shown()[1],
            rows(&["*Calculator", "Text Editor"]),
            "back on its group, the submenu keeps its place"
        );
        shell.send(Event::Motion(shell.middle_of("Lock")));
        assert_eq!(shell.shown().len(), 1, "and shuts for another line");
    }

    #[test]
    fn clicking_a_program_starts_it_and_closes_the_menu() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.click(shell.middle_of("Internet"));
        assert_eq!(shell.shown().len(), 2, "a click on a group opens it");
        let effects = shell.click(shell.middle_of("Firefox"));
        assert_eq!(effects, [Effect::Redraw, Effect::Run(run("firefox"))]);
        assert!(!shell.state.is_open());
    }

    #[test]
    fn escape_closes_the_menu() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.key(Key::Down);
        shell.key(Key::Right);
        assert_eq!(shell.shown().len(), 2);
        assert_eq!(shell.key(Key::Escape), [Effect::Redraw]);
        assert!(!shell.state.is_open(), "all of it, not one level");
        assert_eq!(shell.key(Key::Escape), [], "and nothing is left to close");
    }

    #[test]
    fn a_click_outside_closes_the_menu() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        let inside = shell.middle_of("Lock");
        assert_eq!(
            shell.send(Event::Press((101.0, 101.0))),
            [],
            "the padding is on the menu"
        );
        shell.send(Event::Release((101.0, 101.0)));
        assert!(shell.state.is_open());
        assert_eq!(shell.send(Event::Press((50.0, 50.0))), [Effect::Redraw]);
        assert!(!shell.state.is_open());
        assert_eq!(
            shell.send(Event::Release(inside)),
            [],
            "and the button coming up later chooses nothing"
        );
    }

    #[test]
    fn enter_launches_and_closes() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.key(Key::Down);
        assert_eq!(
            shell.shown(),
            [rows(&["*Accessories", "Internet", "--", "Lock", "Log Out"])]
        );
        shell.key(Key::Enter);
        assert_eq!(
            shell.shown()[1],
            rows(&["*Calculator", "Text Editor"]),
            "Enter on a group goes into it"
        );
        shell.key(Key::Down);
        let effects = shell.key(Key::Enter);
        assert_eq!(effects, [Effect::Redraw, Effect::Run(run("gedit"))]);
        assert!(!shell.state.is_open());
    }

    #[test]
    fn the_arrows_skip_separators_wrap_and_go_in_and_out_of_submenus() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.key(Key::Up);
        assert_eq!(
            shell.shown()[0][4],
            "*Log Out",
            "up from nothing is the last"
        );
        shell.key(Key::Down);
        assert_eq!(shell.shown()[0][0], "*Accessories", "and wraps");
        shell.key(Key::Down);
        shell.key(Key::Down);
        assert_eq!(shell.shown()[0][3], "*Lock", "past the separator");
        shell.key(Key::Home);
        shell.key(Key::Right);
        assert_eq!(shell.shown()[1], rows(&["*Calculator", "Text Editor"]));
        shell.key(Key::End);
        assert_eq!(
            shell.shown()[1],
            rows(&["Calculator", "*Text Editor"]),
            "in the submenu"
        );
        shell.key(Key::Left);
        assert_eq!(
            shell.shown(),
            [rows(&["*Accessories", "Internet", "--", "Lock", "Log Out"])]
        );
        shell.key(Key::Left);
        assert!(shell.state.is_open(), "Left at the root closes nothing");
    }

    #[test]
    fn typing_narrows_the_menu() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.key(Key::Text("c".to_owned()));
        assert_eq!(
            shell.shown(),
            [rows(&["*Calculator", "Lock"])],
            "programs from every group, those starting with it first"
        );
        assert_eq!(shell.state.view().unwrap().query, Some("c"));
        shell.key(Key::Text("A".to_owned()));
        assert_eq!(shell.shown(), [rows(&["*Calculator"])]);
        shell.key(Key::Text("z".to_owned()));
        assert_eq!(shell.shown(), [Vec::<String>::new()], "nothing found");
        assert_eq!(shell.key(Key::Enter), [], "and nothing to choose");
        shell.key(Key::Backspace);
        assert_eq!(
            shell.key(Key::Enter),
            [Effect::Redraw, Effect::Run(run("xcalc"))]
        );

        let mut again = Shell::new();
        again.right_click((100.0, 100.0));
        again.key(Key::Text("f".to_owned()));
        again.key(Key::Backspace);
        assert_eq!(
            again.shown(),
            [rows(&["Accessories", "Internet", "--", "Lock", "Log Out"])],
            "Backspace to nothing is the whole menu again"
        );
        assert_eq!(
            again.key(Key::Text(" ".to_owned())),
            [],
            "a space first types nothing"
        );
    }

    #[test]
    fn typing_shows_no_more_than_fit_in_one_column() {
        let apps: Vec<App> = (0..40)
            .map(|n| app(&format!("tool{n}"), &format!("Tool {n:02}"), "Utility"))
            .collect();
        let mut state = State::new(root(&apps, None, &Session::default()));
        // Room for the typed line and four more.
        let short = Rect::new(0, 0, 800, 5 * menu::ROW + 2 * (menu::PAD + menu::BORDER));
        state.update(
            Event::RootMenu {
                output: "DP-1".to_owned(),
                area: short,
                at: (0, 0),
            },
            &mut Monospace(8.0),
        );
        state.update(Event::Key(Key::Text("t".to_owned())), &mut Monospace(8.0));
        let view = state.view().unwrap();
        let labels: Vec<_> = view.menus[0]
            .lines
            .iter()
            .map(|line| line.item.label.as_str())
            .collect();
        assert_eq!(labels, ["Tool 00", "Tool 01", "Tool 02", "Tool 03"]);
    }

    #[test]
    fn log_out_asks_perspicax_and_losing_the_keyboard_closes() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        assert_eq!(
            shell.click(shell.middle_of("Log Out")),
            [Effect::Redraw, Effect::LogOut]
        );

        shell.right_click((100.0, 100.0));
        assert_eq!(shell.send(Event::KeyboardLost), [Effect::Redraw]);
        assert!(!shell.state.is_open());
    }

    #[test]
    fn an_assistive_technology_chooses_by_route() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        assert_eq!(
            shell.send(Event::Choose(vec![0, 0])),
            [],
            "not shown, so not chosen"
        );
        shell.send(Event::Choose(vec![0]));
        shell.send(Event::Select(vec![0, 1]));
        assert_eq!(shell.shown()[1], rows(&["Calculator", "*Text Editor"]));
        assert_eq!(
            shell.send(Event::Choose(vec![0, 0])),
            [Effect::Redraw, Effect::Run(run("xcalc"))]
        );
    }

    #[test]
    fn the_view_says_which_line_has_the_keyboard_and_which_opened_a_menu() {
        let mut shell = Shell::new();
        shell.right_click((100.0, 100.0));
        shell.key(Key::Down);
        shell.key(Key::Right);
        let view = shell.state.view().unwrap();
        let accessories = &view.menus[0].lines[0];
        assert!(accessories.selected && accessories.open && !accessories.focused);
        let calculator = &view.menus[1].lines[0];
        assert!(calculator.selected && calculator.focused && !calculator.open);
        assert_eq!(calculator.route, [0, 0]);
        assert_eq!(
            view.menus[1].opened_by.map(|item| item.label.as_str()),
            Some("Accessories")
        );
    }
}
