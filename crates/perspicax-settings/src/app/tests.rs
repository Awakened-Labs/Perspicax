//! The window, driven headless as a screen reader would find it: by what
//! each widget is called. The compositor's channel is a stand-in that hears
//! what each test says was pressed, and the judge a closure; the file is a
//! real one, in a folder of the test's own.
//!
//! No fonts are loaded, so text is drawn as nothing; every widget is still
//! laid out and found by its words.

use std::{
    cell::RefCell,
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use eframe::egui;
use egui_kittest::{Harness, kittest::Queryable as _};
use perspicax_config::Trigger;
use perspicax_policy::{Chord, Keysym, Mods};
use perspicax_settings::{Document, Pressed};

use crate::{
    capture::{Channel, Heard, NOT_OFFERED},
    window::{App, Judge},
};

/// The compositor, as a test plays it.
#[derive(Clone, Default)]
struct Compositor(Rc<RefCell<Played>>);

#[derive(Default)]
struct Played {
    asked: usize,
    waiting: bool,
    pressed: VecDeque<Heard>,
}

impl Compositor {
    /// The next thing the person presses, heard once a capture waits.
    fn press(&self, heard: Heard) {
        self.0.borrow_mut().pressed.push_back(heard);
    }

    fn asked(&self) -> usize {
        self.0.borrow().asked
    }

    fn waiting(&self) -> bool {
        self.0.borrow().waiting
    }
}

impl Channel for Compositor {
    fn ask(&mut self) {
        let mut state = self.0.borrow_mut();
        state.asked += 1;
        state.waiting = true;
    }

    fn stop(&mut self) {
        self.0.borrow_mut().waiting = false;
    }

    fn heard(&mut self) -> Option<Heard> {
        let mut state = self.0.borrow_mut();
        if !state.waiting {
            return None;
        }
        let heard = state.pressed.pop_front()?;
        state.waiting = false;
        Some(heard)
    }
}

/// A fresh folder for one test, emptied when it ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "perspicax-settings-app-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch folder");
        Self(dir)
    }

    /// `config.toml` in it, holding `text`.
    fn config(&self, text: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        fs::write(&path, text).expect("a config");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn takes() -> Judge {
    Box::new(|_: &Path| Ok(()))
}

/// The window on the file at `path`, with `compositor` for its channel, or
/// none.
fn window(path: &Path, judge: Judge, compositor: Option<&Compositor>) -> Harness<'static, App> {
    let capture = match compositor {
        Some(compositor) => Ok(Box::new(compositor.clone()) as Box<dyn Channel>),
        None => Err(NOT_OFFERED.to_owned()),
    };
    let app = App::new(Document::open(path).expect("the file"), judge, capture);
    let mut harness = Harness::builder()
        .with_size([900.0, 900.0])
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    harness.run();
    harness
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("the file")
}

fn chord(text: &str) -> Heard {
    let trigger = perspicax_config::trigger(text).expect("a chord");
    Heard::Pressed(Pressed::Chord {
        trigger,
        written: perspicax_config::spell_trigger(trigger).expect("a spelling"),
    })
}

/// Choose "Start a program", and write `command` for it.
fn start(harness: &mut Harness<'_, App>, command: &str) {
    harness.get_by_label("Start a program").click();
    harness.run();
    let field = harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text(command);
    harness.run();
}

#[test]
fn capturing_logo_return_binds_it_in_the_file() {
    let scratch = Scratch::new("logo-return");
    let path = scratch.config("# mine\nprofile = \"classic\"\n");
    let compositor = Compositor::default();
    let mut harness = window(&path, takes(), Some(&compositor));

    harness.get_by_label("Add a binding…").click();
    harness.run();
    assert_eq!(compositor.asked(), 1);
    harness.get_by_label("Press the chord. Escape on its own gives up.");
    compositor.press(chord("Logo+Return"));
    harness.run();
    harness.get_by_label("Logo+Return: what should it do?");
    start(&mut harness, "foot -e 'htop -d 5'");
    harness.get_by_label("Save").click();
    harness.run();

    assert_eq!(
        read(&path),
        "# mine\nprofile = \"classic\"\n\n[keys]\n\
         \"Logo+Return\" = { spawn = [\"foot\", \"-e\", \"htop -d 5\"] }\n"
    );
    harness.get_by_label("Written. The session reads it now.");
    harness.get_by_label("Logo+Return: spawn foot -e 'htop -d 5'");
    assert_eq!(
        harness.query_all_by_label("yours").count(),
        2,
        "the row, and the binding chosen"
    );
}

#[test]
fn escape_alone_ends_the_capture_and_writes_nothing() {
    let scratch = Scratch::new("escape");
    let path = scratch.config("");
    let compositor = Compositor::default();
    let mut harness = window(&path, takes(), Some(&compositor));

    harness.get_by_label("Add a binding…").click();
    harness.run();
    compositor.press(Heard::Pressed(Pressed::GaveUp));
    harness.run();
    assert!(harness.query_by_label("A new binding.").is_none());
    assert!(!compositor.waiting());
    assert_eq!(read(&path), "");
}

#[test]
fn a_key_no_config_can_write_says_not_bindable_yet() {
    let scratch = Scratch::new("keypad");
    let path = scratch.config("");
    let compositor = Compositor::default();
    let mut harness = window(&path, takes(), Some(&compositor));

    harness.get_by_label("Add a binding…").click();
    harness.run();
    compositor.press(Heard::Pressed(Pressed::NotBindable(Keysym::KP_Enter)));
    harness.run();
    harness.get_by_label_contains("KP_Enter can't be written in a config yet");
    harness.get_by_label("Press it instead").click();
    harness.run();
    assert_eq!(compositor.asked(), 2, "pressing another asks again");
    assert_eq!(read(&path), "");
}

#[test]
fn the_app_never_writes_a_file_the_check_refuses() {
    let scratch = Scratch::new("judged");
    let path = scratch.config("");
    let compositor = Compositor::default();
    let asked = Rc::new(RefCell::new(0));
    let judge: Judge = {
        let asked = Rc::clone(&asked);
        Box::new(move |candidate: &Path| {
            if candidate.ends_with("config.toml") {
                return Ok(());
            }
            *asked.borrow_mut() += 1;
            Err("perspicax-shell refuses it: `shell.tray`: no tray here".to_owned())
        })
    };
    let mut harness = window(&path, judge, Some(&compositor));

    harness.get_by_label("Add a binding…").click();
    harness.run();
    compositor.press(chord("Logo+t"));
    harness.run();
    start(&mut harness, "foot");
    harness.get_by_label("Save").click();
    harness.run();

    assert_eq!(*asked.borrow(), 1, "the candidate was judged");
    assert_eq!(read(&path), "");
    harness.get_by_label("Not written.");
    harness.get_by_label("perspicax-shell refuses it: `shell.tray`: no tray here");
    let names: Vec<String> = fs::read_dir(&scratch.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["config.toml"], "no candidate left behind");
}

/// A window with no capture, a new binding begun, and `text` typed for it.
fn typed(scratch: &Scratch, text: &str) -> Harness<'static, App> {
    let mut harness = window(&scratch.config(""), takes(), None);
    harness.get_by_label("Add a binding…").click();
    harness.run();
    harness.get_by_label(NOT_OFFERED);
    let field = harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text(text);
    harness.run();
    harness.get_by_label("Use").click();
    harness.run();
    harness
}

#[test]
fn a_typed_chord_is_read_as_the_config_reads_it() {
    let scratch = Scratch::new("typed");
    typed(&scratch, "Hyper+a").get_by_label_contains("`Hyper`");
    typed(&scratch, "Ctrl+Alt+F2")
        .get_by_label("Ctrl+Alt+F2 switches to VT 2; nothing can bind it.");
    typed(&scratch, "super+shift+a").get_by_label("Logo+Shift+a: what should it do?");
    assert_eq!(read(&scratch.0.join("config.toml")), "");
}

#[test]
fn moving_a_profile_binding_writes_none_for_the_old_chord() {
    let scratch = Scratch::new("move");
    let path = scratch.config("");
    let compositor = Compositor::default();
    let mut harness = window(&path, takes(), Some(&compositor));

    harness.get_by_label("Alt+F4").click();
    harness.run();
    harness.get_by_label("Alt+F4: close");
    harness.get_by_label("Move to another chord…").click();
    harness.run();
    compositor.press(chord("Logo+F4"));
    harness.run();

    let written = read(&path);
    let config = perspicax_config::parse(&written, crate::window::EVERYTHING).unwrap();
    let alt = Mods::alt();
    let logo = Mods {
        logo: true,
        ..Mods::default()
    };
    assert_eq!(
        config.bindings.resolve(alt, &[Keysym::F4]),
        None,
        "{written}"
    );
    assert_eq!(
        config.bindings.resolve(logo, &[Keysym::F4]),
        Some(&perspicax_policy::Action::Close),
        "{written}"
    );
    assert!(written.contains("\"Alt+F4\" = \"none\""), "{written}");
    let rows = perspicax_settings::bindings(&written).unwrap();
    let moved = rows
        .iter()
        .find(|row| {
            row.trigger
                == Trigger::Chord(Chord {
                    mods: logo,
                    key: Keysym::F4,
                })
        })
        .expect("Logo+F4 is a row");
    assert_eq!(moved.written_as, "Logo+F4");
}

#[test]
fn a_refused_file_offers_its_fixes_and_taking_one_writes_it() {
    let scratch = Scratch::new("fixes");
    let refused = "profile = \"minimal\"\n\n[shell.panel]\nclock = \"%H:%M\"\n";
    let path = scratch.config(refused);
    let mut harness = window(&path, takes(), Some(&Compositor::default()));

    harness.get_by_label("perspicax refuses this file as it stands.");
    assert!(harness.query_by_label("Add a binding…").is_none());
    harness
        .get_by_label("Turn the panel on: `enabled = true` in `[shell.panel]`")
        .click();
    harness.run();

    let fixed = read(&path);
    assert!(fixed.contains("enabled = true"), "{fixed}");
    assert!(fixed.contains("clock = \"%H:%M\""), "{fixed}");
    assert!(
        harness
            .query_by_label("perspicax refuses this file as it stands.")
            .is_none()
    );
    harness.get_by_label("Add a binding…");
}

#[test]
fn a_file_that_is_not_toml_offers_only_the_editor() {
    let scratch = Scratch::new("not-toml");
    let path = scratch.config("[keys\n");
    let harness = window(&path, takes(), Some(&Compositor::default()));

    harness.get_by_label("This file is not TOML, so nothing is written into it.");
    harness.get_by_label("Open in an editor");
    assert!(harness.query_by_label("Add a binding…").is_none());
    assert!(harness.query_by_label("Alt+F4").is_none());
    assert_eq!(read(&path), "[keys\n");
}

#[test]
fn a_file_changed_in_an_editor_is_read_again_when_the_window_gets_the_keyboard_back() {
    let scratch = Scratch::new("reload");
    let path = scratch.config("");
    let mut harness = window(&path, takes(), Some(&Compositor::default()));
    assert!(harness.query_by_label("Logo+t").is_none());

    harness.input_mut().focused = false;
    harness.run();
    fs::write(&path, "[keys]\n\"Logo+t\" = { spawn = [\"foot\"] }\n").unwrap();
    harness.input_mut().focused = true;
    harness.run();
    harness.get_by_label("Logo+t");
}
