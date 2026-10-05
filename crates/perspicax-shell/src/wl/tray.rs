//! The tray's glue: its thread on the session bus, running while a panel
//! holds a tray; what it reads, shown on the panels; and a press on one of
//! its icons, asked of the icon's program, whose menu opens beside the icon
//! on the menus' own surface once the program has given it.
//!
//! A menu comes some time after the press that asked for it. It opens only
//! if nothing else was asked for meanwhile: a press on another icon, or
//! another menu opened, makes it too late to show.

use perspicax_config::{Edge, Item, Shell};
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::App;
use crate::{
    layout::Rect,
    model::{Button, menu::Menu, tray::Press},
    tray::{Ask, News, Running},
    update::{Event, Which},
};

/// The tray, as the loop knows it.
pub(super) struct Tray {
    /// The bus it runs on: `None` for the session's.
    bus: Option<String>,
    /// Where its news reaches the loop, marked with which start of it the
    /// news is from.
    news: Sender<(u64, News)>,
    /// Its thread, while a panel holds a tray.
    running: Option<Running>,
    /// How many times it has started: news from an earlier start is from a
    /// tray since stopped.
    started: u64,
    /// The icon last pressed for its menu: its key, the name of the
    /// monitor it was pressed on, and where on that monitor it is.
    pressed: Option<(u64, String, Rect)>,
}

impl Tray {
    pub(super) fn new(bus: Option<String>, news: Sender<(u64, News)>) -> Self {
        Self {
            bus,
            news,
            running: None,
            started: 0,
            pressed: None,
        }
    }

    /// Run the tray if `shell`'s panel holds one, and stop it if not.
    /// Whether it stopped.
    fn follow(&mut self, shell: &Shell) -> bool {
        let wanted = shell
            .panel
            .as_ref()
            .is_some_and(|panel| panel.items.contains(&Item::Tray));
        match (wanted, &self.running) {
            (true, None) => {
                self.started += 1;
                let (news, started) = (self.news.clone(), self.started);
                self.running = Some(crate::tray::start(self.bus.clone(), move |told| {
                    news.send((started, told)).is_ok()
                }));
                false
            }
            (false, Some(_)) => {
                self.running = None;
                self.pressed = None;
                true
            }
            _ => false,
        }
    }

    fn ask(&self, ask: Ask) {
        if let Some(running) = &self.running {
            running.ask(ask);
        }
    }
}

impl App {
    /// Run the tray if the panel `shell` describes holds one, and stop it
    /// if not.
    pub(super) fn follow_tray(&mut self, shell: &Shell) {
        if self.tray.follow(shell) && self.panels.clear_tray() {
            self.panels_changed();
        }
    }

    /// News from start `started` of the tray's thread.
    pub(super) fn tray_news(&mut self, started: u64, news: News) {
        if started != self.tray.started {
            return;
        }
        match news {
            News::Item(key, item) => {
                self.panels.show_status(key, item);
                self.panels_changed();
            }
            News::Gone(key) => {
                if self.panels.forget_status(key) {
                    self.panels_changed();
                }
            }
            News::Menu(key, menu) => self.open_tray_menu(key, menu),
        }
    }

    /// A button went down on the tray's icon of `key`, on the panel of the
    /// monitor named `name`: ask its program for what that is for.
    pub(super) fn press_status(&mut self, name: &str, key: u64, button: Button) {
        let Some(press) = self
            .panels
            .status(key)
            .and_then(|item| item.pressed(button))
        else {
            return;
        };
        let Some(output) = self.output_named(name) else {
            return;
        };
        let Some(place) = self.panels.status_at(name, key, self.monitor(&output)) else {
            return;
        };
        let (x, y) = self
            .outputs
            .info(&output)
            .and_then(|info| info.logical_position)
            .unwrap_or_default();
        let at = (x + place.x + place.w / 2, y + place.y + place.h / 2);
        // An activation a program does not answer opens its menu instead.
        self.tray.pressed =
            matches!(press, Press::Menu | Press::Activate).then(|| (key, name.to_owned(), place));
        self.tray.ask(Ask::Press { key, press, at });
    }

    /// Item `id` of the menu of the tray's icon of `key` was chosen.
    pub(super) fn tell_status(&mut self, key: u64, id: i32) {
        self.tray.ask(Ask::Chosen { key, id });
    }

    /// A menu opened: a tray icon's menu not yet given is too late to show,
    /// unless it was that menu.
    pub(super) fn menu_opened(&mut self, which: Option<Which>) {
        if which.is_some_and(|which| !matches!(which, Which::Tray(_))) {
            self.tray.pressed = None;
        }
    }

    /// The program of the tray's icon of `key` gave its menu: open it beside
    /// the icon, if that is still what was last asked for.
    fn open_tray_menu(&mut self, key: u64, menu: Menu) {
        let Some((_, output, button)) = self.tray.pressed.take_if(|(pressed, ..)| *pressed == key)
        else {
            return;
        };
        let Some(monitor) = self.output_named(&output) else {
            return;
        };
        let area = self.area(&monitor);
        let name = self
            .panels
            .status(key)
            .map(|item| item.title.clone())
            .unwrap_or_default();
        let edge = self.panels.edge().unwrap_or(Edge::Bottom);
        self.menu_event(Event::TrayMenu {
            key,
            name,
            menu,
            output,
            area,
            button,
            edge,
        });
    }
}
