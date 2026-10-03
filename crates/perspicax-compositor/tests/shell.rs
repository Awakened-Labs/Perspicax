//! perspicax-shell, on a compositor with no screen.
//!
//! The shell is an ordinary Wayland client, so it is run here as one: on a
//! thread of this test, connected to a headless compositor, built with every
//! component. What it drew is read back the way an agent reads it, from the
//! facts, and with the `capture` feature from a picture.
//!
//! It puts a wallpaper on every monitor, on a monitor plugged in after it
//! started too, and each one is a surface on the `background` layer named
//! for its monitor. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use std::{path::PathBuf, thread};

use common::{Session, connect};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, Layer, SurfaceKind};
use perspicax_node::Rect;
use perspicax_policy::{Access, Place, Shape, Side};

/// The shell, running on a thread against a session.
struct Shell {
    thread: thread::JoinHandle<Result<(), perspicax_shell::Error>>,
    config: PathBuf,
}

impl Shell {
    /// Start the shell on `session`, reading a config file that holds
    /// `config`.
    fn start(session: &Session, name: &str, config: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "perspicax-shell-{name}-{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, config).expect("a config file");
        let options = perspicax_shell::Options {
            connection: Some(connect(session.socket())),
            config: Some(path.clone()),
        };
        Self {
            thread: thread::spawn(move || perspicax_shell::run(options)),
            config: path,
        }
    }

    /// Stop the session, which stops the shell, as logging out does; and
    /// check that it stopped cleanly rather than failing.
    fn stop_with(self, session: Session) {
        session.stop(());
        let stopped = self.thread.join().expect("the shell thread panicked");
        std::fs::remove_file(&self.config).ok();
        stopped.expect("the shell stopped cleanly with its compositor");
    }
}

/// Two monitors side by side, of different sizes.
fn two_monitors() -> Backend {
    Backend::Headless {
        outputs: vec![
            Virtual::numbered(1, (1280, 1024)),
            Virtual {
                place: Place::Beside {
                    side: Side::RightOf,
                    of: "HEADLESS-1".to_owned(),
                    offset: 0,
                },
                ..Virtual::numbered(2, (1920, 1080))
            },
        ],
        workspaces: Shape::default(),
        access: Access::open(),
    }
}

/// The desktops the facts show, by namespace, with where each is.
fn desktops(facts: &HostFacts) -> Vec<(String, Rect)> {
    let mut desktops: Vec<_> = facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped)
        .filter_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Background,
                namespace,
            } => Some((namespace.clone(), surface.geometry)),
            _ => None,
        })
        .collect();
    desktops.sort_by(|a, b| a.0.cmp(&b.0));
    desktops
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(x + w),
        f64::from(y + h),
    )
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_shell_puts_a_background_surface_on_every_monitor() {
    let session = Session::start("shell-desktops", two_monitors());
    let shell = Shell::start(&session, "desktops", "");

    let facts = session.wait_for(|facts| desktops(facts).len() == 2);
    assert_eq!(
        desktops(&facts),
        [
            (
                "perspicax-desktop-HEADLESS-1".to_owned(),
                rect(0, 0, 1280, 1024)
            ),
            (
                "perspicax-desktop-HEADLESS-2".to_owned(),
                rect(1280, 0, 1920, 1080)
            ),
        ],
        "each monitor covered whole, and named for it"
    );

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_monitor_plugged_in_later_gets_a_wallpaper() {
    let session = Session::start("shell-plug", Backend::headless((1280, 1024)));
    let shell = Shell::start(&session, "plug", "profile = \"minimal\"");
    session.wait_for(|facts| desktops(facts).len() == 1);

    session.command(Command::Plug(Virtual {
        place: Place::Beside {
            side: Side::RightOf,
            of: "HEADLESS-1".to_owned(),
            offset: 0,
        },
        ..Virtual::numbered(2, (1920, 1080))
    }));
    let facts = session.wait_for(|facts| desktops(facts).len() == 2);
    assert_eq!(
        desktops(&facts)[1],
        (
            "perspicax-desktop-HEADLESS-2".to_owned(),
            rect(1280, 0, 1920, 1080)
        )
    );

    session.command(Command::Unplug("HEADLESS-2".to_owned()));
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);
    assert_eq!(
        desktops(&facts)[0].0,
        "perspicax-desktop-HEADLESS-1",
        "and goes when it is unplugged"
    );

    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_of_the_desktop_shows_the_wallpaper_not_the_backdrop() {
    use perspicax_compositor::Host;
    use perspicax_index::ShotTarget;

    let session = Session::start("shell-picture", Backend::headless((800, 600)));
    let shell = Shell::start(&session, "picture", "[shell]\nwallpaper = \"#336699\"");
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);
    let desktop = facts
        .surfaces()
        .iter()
        .find(|surface| matches!(surface.kind, SurfaceKind::Layer { .. }))
        .expect("the desktop")
        .id;

    let host = Host::new(&session.facts, &session.requests);
    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    let pixel = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * shot.width as usize + x) * 4;
        shot.rgba[at..at + 4].try_into().unwrap()
    };
    for (x, y) in [(0, 0), (400, 300), (799, 599)] {
        assert_eq!(
            pixel(x, y),
            [0x33, 0x66, 0x99, 0xff],
            "the wallpaper's colour at {x}, {y}"
        );
    }
    assert!(
        shot.drawn.iter().any(|drawn| drawn.surface == desktop),
        "and the picture says the desktop drew it"
    );

    shell.stop_with(session);
}
