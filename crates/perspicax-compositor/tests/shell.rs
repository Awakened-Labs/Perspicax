//! perspicax-shell, on a compositor with no screen.
//!
//! The shell is an ordinary Wayland client, so it is run here as one: on a
//! thread of this test, connected to a headless compositor, built with every
//! component. What it drew is read back the way an agent reads it, from the
//! facts, and with the `capture` feature from a picture.
//!
//! It puts a wallpaper on every monitor, on a monitor plugged in after it
//! started too, and each one is a surface on the `background` layer named
//! for its monitor. Told its config changed, it repaints those surfaces
//! rather than making new ones. A right-click on the wallpaper, or the root
//! menu's key, opens a menu on the `overlay` layer where the pointer is, and
//! choosing an item in it runs the item's program. In the classic profile a
//! panel along the bottom of each monitor keeps windows above it, and its
//! start button, or the start menu's key, opens the start menu standing on
//! it. Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.
//!
//! The menus here are a menu file's, so that what they hold does not hang
//! on what is installed on the machine running the test.

mod common;

use std::{path::PathBuf, thread};

use common::{Session, connect};
use perspicax_compositor::{Backend, Command, Host, Virtual};
use perspicax_index::{Action as Verb, HostFacts, Layer, PointerButton, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{Access, Action, Place, Shape, Side};

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

    /// Write `config` over the config file, as a person saving it does.
    fn rewrite(&self, config: &str) {
        std::fs::write(&self.config, config).expect("the config file rewritten");
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

/// Every desktop's id, with how many times it has been painted.
fn painted(facts: &HostFacts) -> Vec<(SurfaceId, u64)> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped)
        .filter(|surface| {
            matches!(
                surface.kind,
                SurfaceKind::Layer {
                    layer: Layer::Background,
                    ..
                }
            )
        })
        .map(|surface| (surface.id, surface.damage_generation))
        .collect()
}

/// The menus' surface, if one is up and drawn on: its id, where it is, and
/// where it says it is opaque, which is where the menus are.
fn menu(facts: &HostFacts) -> Option<(SurfaceId, String, Rect, Vec<Rect>)> {
    facts
        .surfaces()
        .iter()
        .find_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Overlay,
                namespace,
            } if surface.mapped => Some((
                surface.id,
                namespace.clone(),
                surface.geometry,
                surface.opaque.clone().filter(|opaque| !opaque.is_empty())?,
            )),
            _ => None,
        })
}

/// The panels the facts show, by namespace, each with its id and where it
/// is.
fn panels(facts: &HostFacts) -> Vec<(String, SurfaceId, Rect)> {
    let mut panels: Vec<_> = facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped)
        .filter_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Top,
                namespace,
            } => Some((namespace.clone(), surface.id, surface.geometry)),
            _ => None,
        })
        .collect();
    panels.sort_by(|a, b| a.0.cmp(&b.0));
    panels
}

/// The id of the surface named `namespace`.
fn layer_named(facts: &HostFacts, namespace: &str) -> SurfaceId {
    facts
        .surfaces()
        .iter()
        .find(|surface| {
            matches!(&surface.kind, SurfaceKind::Layer { namespace: named, .. } if named == namespace)
        })
        .expect("a layer surface of that namespace")
        .id
}

/// The classic profile: a panel along the bottom of every monitor, 40
/// pixels high, with the start button at its left end.
const CLASSIC: &str = "profile = \"classic\"\n";
const PANEL_HEIGHT: f64 = 40.0;

/// The one desktop's id.
fn desktop(facts: &HostFacts) -> SurfaceId {
    facts
        .surfaces()
        .iter()
        .find(|surface| {
            matches!(
                surface.kind,
                SurfaceKind::Layer {
                    layer: Layer::Background,
                    ..
                }
            )
        })
        .expect("the desktop")
        .id
}

/// An agent's click with `button` on `surface`, centred on `x`, `y` of it.
fn click(session: &Session, surface: SurfaceId, (x, y): (f64, f64), button: PointerButton) {
    Host::new(&session.facts, &session.requests)
        .act(
            surface,
            &Verb::Click {
                at: Rect::new(x - 5.0, y - 5.0, x + 5.0, y + 5.0),
                button,
            },
        )
        .expect("dispatched");
}

/// A config whose root menu is one item, `Marker`, that creates `marker`;
/// and the menu file that says so, beside it.
fn marker_menu(name: &str, marker: &std::path::Path) -> (String, PathBuf) {
    let file = std::env::temp_dir().join(format!(
        "perspicax-shell-{name}-menu-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &file,
        format!(
            "mode = \"replace\"\n[[items]]\nlabel = \"Marker\"\nexec = [\"touch\", {:?}]\n",
            marker.display().to_string()
        ),
    )
    .expect("a menu file");
    (
        format!(
            "profile = \"minimal\"\n[shell]\nmenu-file = {:?}\n",
            file.display().to_string()
        ),
        file,
    )
}

/// The middle of `rect`, relative to its own corner's surface.
fn middle(rect: Rect) -> (f64, f64) {
    ((rect.x0 + rect.x1) / 2.0, (rect.y0 + rect.y1) / 2.0)
}

/// The colour of the pixel at `x`, `y` in a picture of the first monitor.
#[cfg(feature = "capture")]
fn colour_at(session: &Session, x: usize, y: usize) -> [u8; 4] {
    use perspicax_index::ShotTarget;

    let host = Host::new(&session.facts, &session.requests);
    let shot = host.capture(ShotTarget::Output(None)).expect("a picture");
    let at = (y * shot.width as usize + x) * 4;
    shot.rgba[at..at + 4].try_into().unwrap()
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
    use perspicax_index::ShotTarget;

    let session = Session::start("shell-picture", Backend::headless((800, 600)));
    // Minimal: no panel along the bottom, so every corner is the wallpaper.
    let shell = Shell::start(
        &session,
        "picture",
        "profile = \"minimal\"\n[shell]\nwallpaper = \"#336699\"",
    );
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

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_reconfigured_shell_changes_its_wallpaper_without_a_new_surface() {
    let session = Session::start("shell-reconfigure", Backend::headless((800, 600)));
    let shell = Shell::start(&session, "reconfigure", "[shell]\nwallpaper = \"#336699\"");
    let facts = session.wait_for(|facts| painted(facts).len() == 1);
    let [(desktop, before)] = painted(&facts)[..] else {
        unreachable!("one desktop, waited for")
    };

    shell.rewrite("[shell]\nwallpaper = \"#996633\"");
    session.command(Command::ReconfigureShell);
    let facts = session.wait_for(|facts| {
        painted(facts)
            .iter()
            .any(|&(id, times)| id == desktop && times > before)
    });
    assert_eq!(
        painted(&facts)
            .iter()
            .map(|&(id, _)| id)
            .collect::<Vec<_>>(),
        [desktop],
        "painted again where it was, and nothing new beside it"
    );
    #[cfg(feature = "capture")]
    assert_eq!(
        colour_at(&session, 400, 300),
        [0x99, 0x66, 0x33, 0xff],
        "in the colour the file says now"
    );

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_right_click_on_the_wallpaper_opens_the_menu_on_the_overlay_layer() {
    let session = Session::start("shell-right-click", Backend::headless((1280, 800)));
    let marker =
        std::env::temp_dir().join(format!("perspicax-shell-unused-{}", std::process::id()));
    let (config, file) = marker_menu("right-click", &marker);
    let shell = Shell::start(&session, "right-click", &config);
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);

    click(
        &session,
        desktop(&facts),
        (300.0, 200.0),
        PointerButton::Right,
    );
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (surface, namespace, geometry, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-1");
    assert_eq!(geometry, rect(0, 0, 1280, 800), "over the whole monitor");
    assert_eq!(opaque.len(), 1, "one menu, and clear around it: {opaque:?}");
    assert_eq!(
        (opaque[0].x0, opaque[0].y0),
        (300.0, 200.0),
        "its corner where the pointer was"
    );

    // It took the keyboard: typing narrows it, which puts what was typed on
    // a line of its own above what it found, and makes it a line taller.
    Host::new(&session.facts, &session.requests)
        .act(
            surface,
            &Verb::Type {
                text: "mark".to_owned(),
            },
        )
        .expect("dispatched");
    let before = opaque[0].y1 - opaque[0].y0;
    session
        .wait_for(|facts| menu(facts).is_some_and(|(_, _, _, now)| now[0].y1 - now[0].y0 > before));

    // A click off the menu, on what is clear of it, closes it.
    click(&session, surface, (100.0, 100.0), PointerButton::Left);
    session.wait_for(|facts| {
        !facts.surfaces().iter().any(|surface| {
            matches!(
                surface.kind,
                SurfaceKind::Layer {
                    layer: Layer::Overlay,
                    ..
                }
            )
        })
    });

    shell.stop_with(session);
    std::fs::remove_file(file).ok();
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn choosing_an_item_runs_its_program_and_closes_the_menu() {
    let session = Session::start("shell-choose", Backend::headless((1280, 800)));
    let marker =
        std::env::temp_dir().join(format!("perspicax-shell-chosen-{}", std::process::id()));
    std::fs::remove_file(&marker).ok();
    let (config, file) = marker_menu("choose", &marker);
    let shell = Shell::start(&session, "choose", &config);
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);

    click(
        &session,
        desktop(&facts),
        (300.0, 200.0),
        PointerButton::Right,
    );
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (surface, _, _, opaque) = menu(&facts).expect("waited for");
    // The menu holds one line, so its middle is the line's.
    click(&session, surface, middle(opaque[0]), PointerButton::Left);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the item's program ran"
        );
        thread::sleep(std::time::Duration::from_millis(20));
    }
    session.wait_for(|facts| menu(facts).is_none());

    shell.stop_with(session);
    std::fs::remove_file(file).ok();
    std::fs::remove_file(marker).ok();
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_root_menu_action_opens_it_at_the_pointer() {
    let session = Session::start("shell-root-action", Backend::headless((1280, 800)));
    let marker =
        std::env::temp_dir().join(format!("perspicax-shell-unused-{}", std::process::id()));
    let (config, file) = marker_menu("root-action", &marker);
    let shell = Shell::start(&session, "root-action", &config);
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);
    // A left click puts the pointer on the wallpaper and opens nothing.
    click(
        &session,
        desktop(&facts),
        (640.0, 400.0),
        PointerButton::Left,
    );
    // The shell has bound perspicax's channel by the time it drew.
    session.perform(Action::RootMenu);

    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (_, _, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!((opaque[0].x0, opaque[0].y0), (640.0, 400.0));

    session.perform(Action::RootMenu);
    session.wait_for(|facts| menu(facts).is_none());

    shell.stop_with(session);
    std::fs::remove_file(file).ok();
}

/// An orange window, with the root menu open over it: the window's id and
/// where it is, and the menus' surface's id and where the menu is.
fn menu_over_a_window(
    session: &Session,
    desk: &mut common::Desk,
    queue: &mut wayland_client::EventQueue<common::Desk>,
    qh: &wayland_client::QueueHandle<common::Desk>,
) -> (SurfaceId, Rect, SurfaceId, Rect) {
    desk.open_coloured(qh, "orange", "orange", 0xffff_8000);
    common::until(queue, desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.mapped && surface.title.as_deref() == Some("orange"))
    });
    let window = facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some("orange"))
        .expect("the window");
    let (window, area) = (window.id, window.geometry);

    // The pointer on the window, and the root menu's key.
    click(session, window, (100.0, 100.0), PointerButton::Left);
    session.perform(Action::RootMenu);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (menu_surface, _, _, opaque) = menu(&facts).expect("waited for");
    let on_menu = opaque[0];
    assert!(
        area.x0 < on_menu.x0
            && on_menu.x0 < area.x1
            && area.y0 < on_menu.y0
            && on_menu.y0 < area.y1,
        "the menu's corner is on the window: {on_menu:?} on {area:?}"
    );
    (window, area, menu_surface, on_menu)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_under_an_open_menu_is_covered_only_where_the_menu_is() {
    use perspicax_index::judge;
    use perspicax_node::Visibility;

    let session = Session::start("shell-menu-covers", Backend::headless((1280, 800)));
    let marker =
        std::env::temp_dir().join(format!("perspicax-shell-unused-{}", std::process::id()));
    let (config, file) = marker_menu("menu-covers", &marker);
    let shell = Shell::start(&session, "menu-covers", &config);
    session.wait_for(|facts| desktops(facts).len() == 1);
    let (mut desk, mut queue, qh, _) = session.client();
    let (window, area, menu_surface, on_menu) =
        menu_over_a_window(&session, &mut desk, &mut queue, &qh);

    // In the window's own coordinates: a button under the menu, and one
    // beside it, in the clear part of the menus' surface.
    let (x, y) = (on_menu.x0 - area.x0, on_menu.y0 - area.y0);
    let facts = session.facts.read();
    assert_eq!(
        judge(
            &facts,
            window,
            Rect::new(x + 5.0, y + 5.0, x + 15.0, y + 15.0)
        )
        .visibility,
        Visibility::Occluded { by: menu_surface },
        "an agent's click under the menu is refused, naming the menu"
    );
    assert_eq!(
        judge(
            &facts,
            window,
            Rect::new(x - 40.0, y - 40.0, x - 30.0, y - 30.0)
        )
        .visibility,
        Visibility::Visible,
        "and one beside it is not: the clear rest of the surface covers nothing"
    );

    drop((desk, queue));
    shell.stop_with(session);
    std::fs::remove_file(file).ok();
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_shows_the_menu_over_a_window() {
    let session = Session::start("shell-menu-picture", Backend::headless((1280, 800)));
    let marker =
        std::env::temp_dir().join(format!("perspicax-shell-unused-{}", std::process::id()));
    let (config, file) = marker_menu("menu-picture", &marker);
    let shell = Shell::start(&session, "menu-picture", &config);
    session.wait_for(|facts| desktops(facts).len() == 1);
    let (mut desk, mut queue, qh, _) = session.client();
    let (_, _, menu_surface, on_menu) = menu_over_a_window(&session, &mut desk, &mut queue, &qh);

    let (x, y) = (on_menu.x0 as usize, on_menu.y0 as usize);
    // Two pixels in: past the border, in the menu's padding.
    assert_eq!(
        colour_at(&session, x + 2, y + 2),
        [0xfc, 0xfc, 0xfc, 0xff],
        "the menu, over the window"
    );
    assert_eq!(
        colour_at(&session, x - 2, y - 2),
        [0xff, 0x80, 0x00, 0xff],
        "and the window beside it, through the clear rest of its surface"
    );
    let shot = Host::new(&session.facts, &session.requests)
        .capture(perspicax_index::ShotTarget::Output(None))
        .expect("a picture");
    assert!(
        shot.drawn.iter().any(|drawn| drawn.surface == menu_surface),
        "and the picture says the menus' surface drew it"
    );

    drop((desk, queue));
    shell.stop_with(session);
    std::fs::remove_file(file).ok();
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_panel_reserves_its_height() {
    let session = Session::start("shell-panel", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "panel", CLASSIC);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (namespace, _, geometry) = &panels(&facts)[0];
    assert_eq!(namespace, "perspicax-panel-HEADLESS-1");
    assert_eq!(
        *geometry,
        rect(0, 760, 1280, 40),
        "along the bottom, the monitor's width"
    );
    // And drawn there: the screen and pictures stack the same way.
    #[cfg(feature = "capture")]
    {
        assert_eq!(
            colour_at(&session, 640, 790),
            [0x23, 0x26, 0x29, 0xff],
            "the panel's colour along the bottom"
        );
        assert_eq!(
            colour_at(&session, 640, 10),
            [0x1e, 0x4a, 0x73, 0xff],
            "and classic's wallpaper at the top"
        );
    }

    // A window maximized is offered what the panel leaves.
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "orange", "orange", 0xffff_8000);
    common::until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.perform(Action::ToggleMaximize);
    common::until(&mut queue, &mut desk, |desk| {
        desk.offered.is_some_and(|(width, _)| width == 1280)
    });
    assert_eq!(
        desk.offered,
        Some((1280, 760)),
        "the monitor less the panel's strip"
    );

    drop((desk, queue));
    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_start_menu_opens_on_the_monitor_under_the_pointer() {
    let session = Session::start("shell-start-key", two_monitors());
    let shell = Shell::start(&session, "start-key", CLASSIC);
    let facts = session.wait_for(|facts| desktops(facts).len() == 2 && panels(facts).len() == 2);

    // The pointer on the second monitor, and the start menu's key.
    click(
        &session,
        layer_named(&facts, "perspicax-desktop-HEADLESS-2"),
        (500.0, 500.0),
        PointerButton::Left,
    );
    session.perform(Action::StartMenu);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (_, namespace, geometry, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-2");
    assert_eq!(geometry, rect(1280, 0, 1920, 1080), "over that monitor");
    assert_eq!(opaque.len(), 1, "one menu: {opaque:?}");
    assert_eq!(
        (opaque[0].x0, opaque[0].y1),
        (0.0, 1080.0 - PANEL_HEIGHT),
        "standing on the panel, at the start button's corner"
    );

    session.perform(Action::StartMenu);
    session.wait_for(|facts| menu(facts).is_none());

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_click_on_the_start_button_opens_it() {
    let session = Session::start("shell-start-click", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "start-click", CLASSIC);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();

    // The start button is the square at the panel's left end.
    let start = (PANEL_HEIGHT / 2.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, start, PointerButton::Left);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (_, namespace, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-1");
    assert_eq!(
        (opaque[0].x0, opaque[0].y1),
        (0.0, 800.0 - PANEL_HEIGHT),
        "above the button"
    );

    // The panel is still in reach with the menu open, and a second click
    // on the button closes it.
    click(&session, panel, start, PointerButton::Left);
    session.wait_for(|facts| menu(facts).is_none());

    shell.stop_with(session);
}
