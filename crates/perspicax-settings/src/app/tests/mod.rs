//! The window, driven headless as a screen reader would find it: by what
//! each widget is called. The compositor's channel is a stand-in that hears
//! what each test says was pressed, and the judge a closure; the file is a
//! real one, in a folder of the test's own.
//!
//! No fonts are loaded, so text is drawn as nothing; every widget is still
//! laid out and found by its words.

mod keys;
mod pages;

use std::{
    cell::RefCell,
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use egui_kittest::Harness;
use perspicax_settings::{Document, Registry};

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
    window_with(path, judge, compositor, None)
}

/// [`window`], with `registry` for xkb's layouts.
fn window_with(
    path: &Path,
    judge: Judge,
    compositor: Option<&Compositor>,
    registry: Option<Registry>,
) -> Harness<'static, App> {
    let capture = match compositor {
        Some(compositor) => Ok(Box::new(compositor.clone()) as Box<dyn Channel>),
        None => Err(NOT_OFFERED.to_owned()),
    };
    let app = App::new(
        Document::open(path).expect("the file"),
        judge,
        capture,
        registry,
    );
    let mut harness = Harness::builder()
        .with_size([900.0, 900.0])
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    harness.run();
    harness
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("the file")
}
