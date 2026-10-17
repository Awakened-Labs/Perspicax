//! perspicax-shell, on a compositor with no screen.
//!
//! The shell is an ordinary Wayland client, so it is run here as one: on a
//! thread of this test, connected to a headless compositor, built with every
//! component. What it drew is read back the way an agent reads it, from the
//! facts, and with the `capture` feature from a picture.
//!
//! It puts a wallpaper on every monitor, on a monitor plugged in after it
//! started too, and each one is a surface on the `background` layer named
//! for its monitor. Cut off from a compositor that runs on, it says it lost
//! its connection, in the exit status perspicax starts it again on. Told its
//! config changed, it repaints those surfaces rather than making new ones.
//! A workspace with a wallpaper of its own
//! shows it while it is the one showing, on whichever monitor shows it.
//! A right-click on the wallpaper, or the root
//! menu's key, opens a menu on the `overlay` layer where the pointer is, and
//! choosing an item in it runs the item's program. In the classic profile a
//! panel along the bottom of each monitor keeps windows above it, and its
//! start button, or the start menu's key, opens the start menu standing on
//! it. The start menu holds, and its button wears, what `[shell.start-menu]`
//! says, and a save changes them while the shell runs. Its taskbar lists the windows, and a click on one brings it forward,
//! puts it away or closes it; its pager shows the workspaces, follows a
//! switch, and switches on a click. Its tray shows a program's status icon
//! once the program registers it, asks the program for what a click on it
//! is for, opens its menu and tells the program what was chosen in it.
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.
//!
//! Each shell has a session bus of its own, a `dbus-daemon` the test starts,
//! so that its tray meets no program of the machine running the test, and
//! no sound server unless the test starts one of its own: PipeWire, its
//! PulseAudio server and WirePlumber's policy, meeting no sound card. A
//! click on the panel's speaker mutes that server's output.
//!
//! The menus here are a menu file's, so that what they hold does not hang
//! on what is installed on the machine running the test.

mod common;

use std::{net::Shutdown, path::PathBuf, thread};

use common::{Desk, Session, connect};
use perspicax_compositor::{ActError, Backend, Command, Host, Keymap, Virtual};
use perspicax_index::{Action as Verb, HostFacts, Layer, PointerButton, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{Access, Action, Ended, Place, Protocol, Rule, Shape, Side};
use wayland_client::Connection;
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

/// The shell, running on a thread against a session.
struct Shell {
    thread: thread::JoinHandle<Result<(), perspicax_shell::Error>>,
    config: PathBuf,
    /// The session bus its tray is on.
    bus: Bus,
}

impl Shell {
    /// Start the shell on `session`, reading a config file that holds
    /// `config`.
    fn start(session: &Session, name: &str, config: &str) -> Self {
        Self::start_on(connect(session.socket()), name, config)
    }

    /// The same, on a connection of the test's own.
    fn start_on(connection: Connection, name: &str, config: &str) -> Self {
        Self::start_with(connection, name, config, None)
    }

    /// The same, its mixer on the sound server at `sound`, or with `None`,
    /// on a socket where none listens.
    fn start_with(
        connection: Connection,
        name: &str,
        config: &str,
        sound: Option<PathBuf>,
    ) -> Self {
        let path = std::env::temp_dir().join(format!(
            "perspicax-shell-{name}-{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, config).expect("a config file");
        let bus = Bus::start();
        let nowhere = std::env::temp_dir().join(format!(
            "perspicax-shell-{name}-{}-no-sound",
            std::process::id()
        ));
        let options = perspicax_shell::Options {
            connection: Some(connection),
            config: Some(path.clone()),
            session_bus: Some(bus.address.clone()),
            sound_server: Some(sound.unwrap_or(nowhere)),
        };
        Self {
            thread: thread::spawn(move || perspicax_shell::run(options)),
            config: path,
            bus,
        }
    }

    /// Write `config` over the config file, as a person saving it does.
    fn rewrite(&self, config: &str) {
        std::fs::write(&self.config, config).expect("the config file rewritten");
    }

    /// Stop the session, which stops the shell, as logging out does; and
    /// check that it stopped for its lost connection rather than failing
    /// some other way. A shell cannot tell its compositor ending from being
    /// cut off, so it says the same of both.
    fn stop_with(self, session: Session) {
        session.stop(());
        let stopped = self.ended();
        assert!(
            matches!(stopped, Err(perspicax_shell::Error::Lost(_))),
            "the shell stopped with its compositor: {stopped:?}"
        );
    }

    /// Wait for the shell to stop, and say how it did.
    fn ended(self) -> Result<(), perspicax_shell::Error> {
        let stopped = self.thread.join().expect("the shell thread panicked");
        std::fs::remove_file(&self.config).ok();
        // The bus outlives the shell, which leaves it as it stops.
        drop(self.bus);
        stopped
    }
}

/// A session bus of the test's own.
struct Bus {
    daemon: std::process::Child,
    address: String,
}

impl Bus {
    fn start() -> Self {
        use std::io::BufRead;

        let mut daemon = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("dbus-daemon, which a session with a bus has");
        let mut address = String::new();
        std::io::BufReader::new(daemon.stdout.take().expect("its output"))
            .read_line(&mut address)
            .expect("its address");
        Self {
            daemon,
            address: address.trim().to_owned(),
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.daemon.kill().ok();
        self.daemon.wait().ok();
    }
}

/// A sound server of the test's own, as a desktop runs one: PipeWire, its
/// PulseAudio server, and WirePlumber's policy alone, so that it meets no
/// sound card and no other program; with two outputs that play nowhere,
/// "one", the default, and "two". Everything it keeps is in a folder of its
/// own under the runtime folder, short enough for a socket's path, and goes
/// with it.
#[cfg(feature = "capture")]
struct SoundServer {
    folder: PathBuf,
    daemons: Vec<std::process::Child>,
}

#[cfg(feature = "capture")]
impl SoundServer {
    fn start(name: &str) -> Self {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let folder =
            PathBuf::from(runtime).join(format!("pspx-sound-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&folder).ok();
        for inside in ["run", "state", "config"] {
            std::fs::create_dir_all(folder.join(inside)).expect("the sound server's folder");
        }
        let mut server = Self {
            folder,
            daemons: Vec::new(),
        };
        server.daemon(&["pipewire"]);
        server.until(|server| server.folder.join("run/pipewire-0").exists());
        server.daemon(&["pipewire", "-c", "pipewire-pulse.conf"]);
        server.daemon(&["wireplumber", "-p", "policy"]);
        server.until(|server| server.socket().exists());
        for sink in ["one", "two"] {
            server.pactl(&[
                "load-module",
                "module-null-sink",
                &format!("sink_name={sink}"),
            ]);
        }
        server.until(|server| server.pactl(&["get-default-sink"]).trim() == "one");
        server
    }

    /// Its PulseAudio server's socket.
    fn socket(&self) -> PathBuf {
        self.folder.join("run/pulse/native")
    }

    /// `program` with the server's folders, and no session bus.
    fn command(&self, program: &str) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command
            .env("XDG_RUNTIME_DIR", self.folder.join("run"))
            .env("XDG_STATE_HOME", self.folder.join("state"))
            .env("XDG_CONFIG_HOME", self.folder.join("config"))
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("PIPEWIRE_REMOTE")
            .env_remove("PULSE_SERVER");
        command
    }

    fn daemon(&mut self, argv: &[&str]) {
        let daemon = self
            .command(argv[0])
            .args(&argv[1..])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("{}, which a desktop with sound has: {error}", argv[0]));
        self.daemons.push(daemon);
    }

    /// What `pactl` says to `args`.
    fn pactl(&self, args: &[&str]) -> String {
        let said = self
            .command("pactl")
            .args(args)
            .output()
            .expect("pactl, which a desktop with sound has");
        String::from_utf8_lossy(&said.stdout).into_owned()
    }

    /// Wait until `ready` holds.
    fn until(&self, ready: impl Fn(&Self) -> bool) {
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready(self) {
            assert!(
                Instant::now() < deadline,
                "the sound server never got ready"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(feature = "capture")]
impl Drop for SoundServer {
    fn drop(&mut self) {
        for daemon in self.daemons.iter_mut().rev() {
            daemon.kill().ok();
            daemon.wait().ok();
        }
        std::fs::remove_dir_all(&self.folder).ok();
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
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
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
                Some(surface.opaque.clone()).filter(|opaque| !opaque.is_empty())?,
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
    colour_on(session, None, x, y)
}

/// The colour of the pixel at `x`, `y` in a picture of the monitor named
/// `output`, or of the first.
#[cfg(feature = "capture")]
fn colour_on(session: &Session, output: Option<&str>, x: usize, y: usize) -> [u8; 4] {
    use perspicax_index::ShotTarget;

    let host = Host::new(&session.facts, &session.requests);
    let shot = host
        .capture(ShotTarget::Output(output.map(str::to_owned)))
        .expect("a picture");
    let at = (y * shot.width as usize + x) * 4;
    shot.rgba[at..at + 4].try_into().unwrap()
}

/// The panel's colours where its tasks and workspaces are: a task's face,
/// and a workspace's not showing; the face of the window with the keyboard,
/// and of the workspace showing; and the bar, where neither is.
#[cfg(feature = "capture")]
const FACE: [u8; 4] = [0x31, 0x36, 0x3b, 0xff];
#[cfg(feature = "capture")]
const LIT: [u8; 4] = [0x2b, 0x4f, 0x63, 0xff];
#[cfg(feature = "capture")]
const BAR: [u8; 4] = [0x23, 0x26, 0x29, 0xff];

/// How wide a task is, with few windows open.
#[cfg(feature = "capture")]
const TASK: f64 = 200.0;

/// Wait until the pixel at `x`, `y` of the first monitor is `colour`: what
/// the panel shows once the shell has heard the news and drawn it.
#[cfg(feature = "capture")]
fn until_colour(session: &Session, at: (usize, usize), colour: [u8; 4]) {
    until_colour_on(session, None, at, colour);
}

/// Wait until the pixel at `x`, `y` of the monitor named `output`, or of
/// the first, is `colour`.
#[cfg(feature = "capture")]
fn until_colour_on(
    session: &Session,
    output: Option<&str>,
    (x, y): (usize, usize),
    colour: [u8; 4],
) {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let now = colour_on(session, output, x, y);
        if now == colour {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "({x}, {y}) is {now:02x?}, not {colour:02x?}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// Whether the window titled `title` has the keyboard, as a taskbar is told.
fn activated(desk: &Desk, title: &str) -> bool {
    desk.task(title)
        .is_some_and(|task| task.is(State::Activated))
}

/// One 1280 by 800 monitor, and four workspaces in a row across it.
#[cfg(feature = "capture")]
fn four_workspaces() -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (1280, 800))],
        workspaces: Shape {
            mode: perspicax_policy::Mode::Spanning,
            grid: perspicax_policy::Grid {
                columns: 4,
                rows: 1,
                wrap: false,
            },
        },
        access: Access::open(),
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

/// Two monitors side by side, each with four workspaces of its own in a
/// row.
#[cfg(feature = "capture")]
fn two_monitors_each_its_own() -> Backend {
    let Backend::Headless {
        outputs, access, ..
    } = two_monitors()
    else {
        unreachable!("two headless monitors")
    };
    Backend::Headless {
        outputs,
        workspaces: Shape {
            mode: perspicax_policy::Mode::PerOutput,
            grid: perspicax_policy::Grid {
                columns: 4,
                rows: 1,
                wrap: false,
            },
        },
        access,
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

/// A minimal desktop of the colour `SHELLS`, with `OWN` for workspace 2
/// and `TILED` for workspace 4: written as a table, with a mode, which a
/// colour fills the monitor in whatever it is.
#[cfg(feature = "capture")]
const OWN_WALLPAPERS: &str = "profile = \"minimal\"\n[shell]\nwallpaper = \"#336699\"\n\
    [shell.wallpapers]\n2 = \"#996633\"\n4 = { wallpaper = \"#669933\", mode = \"tile\" }\n";
#[cfg(feature = "capture")]
const SHELLS: [u8; 4] = [0x33, 0x66, 0x99, 0xff];
#[cfg(feature = "capture")]
const OWN: [u8; 4] = [0x99, 0x66, 0x33, 0xff];
#[cfg(feature = "capture")]
const TILED: [u8; 4] = [0x66, 0x99, 0x33, 0xff];

/// A panel of the pager and the clock alone, so the pager is at its left
/// end whatever the clock's font makes its width.
#[cfg(feature = "capture")]
const PAGER_FIRST: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"pager\", \"clock\"]\n";

/// The middle of the left end of workspace `at`'s cell in a pager at the
/// left end of a 40 pixel panel, clear of its name: the cells are 51 by 32,
/// 4 in from the panel's edges and 2 apart.
#[cfg(feature = "capture")]
fn cell(at: usize) -> (f64, f64) {
    (f64::from(4 + 53 * at as i32 + 4), 20.0)
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

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shell_cut_off_while_perspicax_runs_says_it_lost_the_connection() {
    let session = Session::start("shell-cut-off", two_monitors());
    let socket = common::stream(session.socket());
    let cut = socket.try_clone().expect("a second handle on the socket");
    let shell = Shell::start_on(
        Connection::from_socket(socket).expect("a Wayland connection"),
        "cut-off",
        "",
    );
    session.wait_for(|facts| desktops(facts).len() == 2);

    // From the shell's side, wayland-backend cutting off a client that
    // stopped reading looks like this: the connection just ends.
    cut.shutdown(Shutdown::Both).expect("the connection cut");
    let lost = shell
        .ended()
        .expect_err("a shell cut off does not stop cleanly");
    assert!(
        matches!(lost, perspicax_shell::Error::Lost(_)),
        "it says it lost its connection: {lost:?}"
    );
    assert_eq!(
        Ended::from_code(Some(i32::from(lost.exit_code()))),
        Ended::Lost,
        "with an exit status perspicax starts it again on"
    );
    session.wait_for(|facts| desktops(facts).is_empty());
    assert!(!session.ended(), "perspicax runs on, to read it");

    session.stop(());
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

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_workspace_switch_shows_its_wallpaper() {
    let session = Session::start("shell-wallpapers", four_workspaces());
    let shell = Shell::start(&session, "wallpapers", OWN_WALLPAPERS);
    let facts = session.wait_for(|facts| painted(facts).len() == 1);
    let [(desktop, _)] = painted(&facts)[..] else {
        unreachable!("one desktop, waited for")
    };
    let middle = (640, 400);
    until_colour(&session, middle, SHELLS);

    session.perform(Action::GoToWorkspace(2));
    until_colour(&session, middle, OWN);
    session.perform(Action::GoToWorkspace(3));
    until_colour(&session, middle, SHELLS);
    session.perform(Action::GoToWorkspace(4));
    until_colour(&session, middle, TILED);
    session.perform(Action::GoToWorkspace(1));
    until_colour(&session, middle, SHELLS);

    let facts = session.wait_for(|_| true);
    assert_eq!(
        painted(&facts)
            .iter()
            .map(|&(id, _)| id)
            .collect::<Vec<_>>(),
        [desktop],
        "painted again where it was, and nothing new beside it"
    );

    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn each_monitor_shows_the_wallpaper_of_the_workspace_it_shows() {
    let session = Session::start("shell-wallpapers-each", two_monitors_each_its_own());
    let shell = Shell::start(&session, "wallpapers-each", OWN_WALLPAPERS);
    let facts = session.wait_for(|facts| desktops(facts).len() == 2);
    let corner = (100, 100);
    until_colour_on(&session, Some("HEADLESS-1"), corner, SHELLS);
    until_colour_on(&session, Some("HEADLESS-2"), corner, SHELLS);

    // The pointer on the second monitor, which a switch then switches.
    click(
        &session,
        layer_named(&facts, "perspicax-desktop-HEADLESS-2"),
        (500.0, 500.0),
        PointerButton::Left,
    );
    session.perform(Action::GoToWorkspace(2));
    until_colour_on(&session, Some("HEADLESS-2"), corner, OWN);
    assert_eq!(
        colour_on(&session, Some("HEADLESS-1"), corner.0, corner.1),
        SHELLS,
        "the first monitor is on its own workspace 1 still"
    );

    shell.stop_with(session);
}

/// Issue #172: a monitor plugged in joins the workspaces spanning the
/// others, and shows the wallpaper of the one showing. The shell learns
/// which that is only by being told, on the monitor it bound, that the
/// group is on it; before, it was told before it could bind it, and the new
/// monitor showed the shell's own wallpaper whatever the workspace.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_monitor_plugged_in_later_shows_the_wallpaper_of_the_workspace_spanning_it() {
    let session = Session::start("shell-plug-wallpaper", four_workspaces());
    let shell = Shell::start(&session, "plug-wallpaper", OWN_WALLPAPERS);
    session.wait_for(|facts| desktops(facts).len() == 1);
    session.perform(Action::GoToWorkspace(2));
    until_colour_on(&session, Some("HEADLESS-1"), (100, 100), OWN);

    session.command(Command::Plug(Virtual {
        place: Place::Beside {
            side: Side::RightOf,
            of: "HEADLESS-1".to_owned(),
            offset: 0,
        },
        ..Virtual::numbered(2, (1280, 800))
    }));
    session.wait_for(|facts| desktops(facts).len() == 2);
    until_colour_on(&session, Some("HEADLESS-2"), (100, 100), OWN);

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

/// The pie's surface, if one is up: its id, its namespace and where it is.
/// It says it is opaque nowhere, so it is told from the menus' by its name.
fn pie(facts: &HostFacts) -> Option<(SurfaceId, String, Rect)> {
    facts
        .surfaces()
        .iter()
        .find_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Overlay,
                namespace,
            } if surface.mapped && namespace.starts_with("perspicax-pie-") => {
                Some((surface.id, namespace.clone(), surface.geometry))
            }
            _ => None,
        })
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_pie_opens_at_the_pointer_and_a_slot_pointed_at_runs_its_program() {
    let session = Session::start("shell-pie", Backend::headless((1280, 800)));
    let marker = std::env::temp_dir().join(format!("perspicax-shell-pie-{}", std::process::id()));
    std::fs::remove_file(&marker).ok();
    let config = format!(
        "profile = \"minimal\"\n[shell.pie.menus]\n\
         t = [{{ label = \"Marker\", exec = [\"touch\", {:?}] }}]\n",
        marker.display().to_string()
    );
    let shell = Shell::start(&session, "pie", &config);
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);
    // A left click puts the pointer on the wallpaper and opens nothing.
    click(
        &session,
        desktop(&facts),
        (640.0, 400.0),
        PointerButton::Left,
    );

    session.perform(Action::Pie("t".to_owned()));
    let facts = session.wait_for(|facts| pie(facts).is_some());
    let (surface, namespace, area) = pie(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-pie-HEADLESS-1");
    assert_eq!(
        (area.x0, area.y0, area.x1, area.y1),
        (0.0, 0.0, 1280.0, 800.0),
        "over the whole monitor"
    );
    // Asked again, it closes; and opens once more.
    session.perform(Action::Pie("t".to_owned()));
    session.wait_for(|facts| pie(facts).is_none());
    session.perform(Action::Pie("t".to_owned()));
    let facts = session.wait_for(|facts| pie(facts).is_some());
    let (surface_again, _, _) = pie(&facts).expect("waited for");
    assert_ne!(surface, surface_again, "a surface of its own each time");

    // Its one slot is at the top; pointing straight up, out at the edge,
    // picks it, and a click there runs it and closes the pie.
    click(&session, surface_again, (640.0, 6.0), PointerButton::Left);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the slot's program ran"
        );
        thread::sleep(std::time::Duration::from_millis(20));
    }
    session.wait_for(|facts| pie(facts).is_none());

    shell.stop_with(session);
    std::fs::remove_file(marker).ok();
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_pies_running_application_brings_its_next_window_forward_each_time() {
    let session = Session::start("shell-pie-running", Backend::headless((1280, 800)));
    let config = "profile = \"minimal\"\n[shell.pie.menus]\nt = [{ running = true }]\n";
    let shell = Shell::start(&session, "pie-running", config);
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "slack");
    desk.open_window(&qh, "second", "slack");
    common::until(&mut queue, &mut desk, |desk| {
        desk.task("first").is_some() && activated(desk, "second")
    });
    // The pointer on the wallpaper, clear of the windows.
    click(&session, desktop(&facts), (20.0, 20.0), PointerButton::Left);

    // One slot, slack's, standing for both windows: choosing it brings
    // forward the one after the window that has the keyboard, and again.
    for next in ["first", "second", "first"] {
        session.perform(Action::Pie("t".to_owned()));
        let facts = session.wait_for(|facts| pie(facts).is_some());
        let (surface, _, _) = pie(&facts).expect("waited for");
        // Kept inside the monitor, the pie's middle is at (256, 256); its
        // one slot is at the top, and straight up from there picks it.
        click(&session, surface, (256.0, 6.0), PointerButton::Left);
        session.wait_for(|facts| pie(facts).is_none());
        common::until(&mut queue, &mut desk, |desk| activated(desk, next));
    }

    shell.stop_with(session);
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
fn an_open_menu_covers_the_window_under_it_everywhere_and_proves_it_under_the_menu() {
    use perspicax_index::{Judgement, judge};
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
        ),
        Judgement {
            visibility: Visibility::Occluded { by: menu_surface },
            unproven: false,
        },
        "an agent's click under the menu is refused, naming the menu, on the \
         shell's word that the menu is opaque"
    );
    assert_eq!(
        judge(
            &facts,
            window,
            Rect::new(x - 40.0, y - 40.0, x - 30.0, y - 30.0)
        ),
        Judgement {
            visibility: Visibility::Occluded { by: menu_surface },
            unproven: true,
        },
        "and one beside it too, on policy: the clear rest of the surface is \
         drawn over it, and a click there lands on it and only closes the menu"
    );
    drop(facts);

    // The menu holds the keyboard, so typing at the window under it would
    // type into the menu: refused, naming the menu.
    assert_eq!(
        Host::new(&session.facts, &session.requests).act(
            window,
            &Verb::Type {
                text: "x".to_owned()
            }
        ),
        Err(ActError::FocusElsewhere {
            focused: Some(menu_surface)
        })
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

/// The panel is drawn in the theme's colours, and a theme saved while the
/// shell runs reaches it in place.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_panel_wears_the_theme_and_one_saved_later_in_place() {
    use perspicax_policy::{Builtin, Role};

    let session = Session::start("shell-theme", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "theme", "[theme.palette]\npanel = \"#102030\"\n");
    session.wait_for(|facts| panels(facts).len() == 1);
    until_colour(&session, (640, 790), [0x10, 0x20, 0x30, 0xff]);

    shell.rewrite("[theme]\nname = \"breeze-dark\"\n");
    session.command(Command::ReconfigureShell);
    let dark = Builtin::BreezeDark.palette()[Role::Panel];
    until_colour(&session, (640, 790), [dark.r, dark.g, dark.b, 0xff]);

    shell.stop_with(session);
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

/// Classic's panel, `width` wide along the bottom, at `align`.
fn narrow(width: &str, align: &str) -> String {
    format!("{CLASSIC}[shell.panel]\nwidth = {width}\nalign = \"{align}\"\n")
}

/// The one panel's id, where it is, and its bar: where it says it is
/// opaque, once it has drawn.
fn bar(facts: &HostFacts) -> Option<(SurfaceId, Rect, Rect)> {
    facts
        .surfaces()
        .iter()
        .find_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Top, ..
            } if surface.mapped => match surface.opaque.as_slice() {
                [bar] => Some((surface.id, surface.geometry, *bar)),
                _ => None,
            },
            _ => None,
        })
}

/// Wait until the one panel's bar is `wanted`, and say what the facts were.
fn until_bar(session: &Session, wanted: Rect) -> (SurfaceId, Rect) {
    let facts = session.wait_for(|facts| bar(facts).is_some_and(|(_, _, at)| at == wanted));
    let (id, geometry, _) = bar(&facts).expect("waited for");
    (id, geometry)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrow_panel_keeps_windows_out_of_its_whole_strip() {
    let session = Session::start("shell-narrow", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "narrow", &narrow("600", "left"));
    let (_, geometry) = until_bar(&session, rect(0, 0, 600, 40));
    assert_eq!(
        geometry,
        rect(0, 760, 1280, 40),
        "the panel is the whole strip, its bar only the left of it"
    );

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
        "the monitor less the whole strip, beside the bar too"
    );

    drop((desk, queue));
    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrow_panels_start_button_opens_the_start_menu_above_it() {
    let session = Session::start("shell-narrow-start", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "narrow-start", &narrow("600", "center"));
    let (panel, _) = until_bar(&session, rect(340, 0, 600, 40));

    let start = (340.0 + PANEL_HEIGHT / 2.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, start, PointerButton::Left);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (_, _, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(
        (opaque[0].x0, opaque[0].y1),
        (340.0, 800.0 - PANEL_HEIGHT),
        "above the button, where the bar put it"
    );
    click(&session, panel, start, PointerButton::Left);
    session.wait_for(|facts| menu(facts).is_none());

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_saved_width_and_align_move_the_bar_in_place() {
    let session = Session::start("shell-narrow-saved", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "narrow-saved", &narrow("600", "center"));
    let (before, _) = until_bar(&session, rect(340, 0, 600, 40));

    shell.rewrite(&narrow("600", "right"));
    session.command(Command::ReconfigureShell);
    let (after, _) = until_bar(&session, rect(680, 0, 600, 40));
    assert_eq!(after, before, "the same panel, redrawn");

    shell.rewrite(&narrow("\"50%\"", "left"));
    session.command(Command::ReconfigureShell);
    let (after, _) = until_bar(&session, rect(0, 0, 640, 40));
    assert_eq!(after, before);

    shell.rewrite(CLASSIC);
    session.command(Command::ReconfigureShell);
    until_bar(&session, rect(0, 0, 1280, 40));

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrow_panel_shrinks_its_tasks_to_their_icons_then_grows() {
    /// A task's icon alone, as the shell lays out a task it cannot shrink
    /// further.
    const ICON: i32 = 36;

    let session = Session::start("shell-narrow-grows", Backend::headless((1280, 800)));
    let config = format!(
        "{CLASSIC}[shell.panel]\nwidth = 120\nitems = [\"start\", \"taskbar\", \"clock\"]\n"
    );
    let shell = Shell::start(&session, "narrow-grows", &config);
    until_bar(&session, rect(580, 0, 120, 40));

    // Start, a task and the clock are already more than 120 pixels: each
    // window's icon widens the bar, in the middle still.
    let (mut desk, mut queue, qh, _) = session.client();
    let mut widths = vec![120.0];
    for (open, title) in ["first", "second"].into_iter().enumerate() {
        desk.open_coloured(&qh, title, title, 0xffff_8000);
        common::until(&mut queue, &mut desk, |desk| desk.drawn == open + 1);
        let narrower = widths[open];
        let facts =
            session.wait_for(|facts| bar(facts).is_some_and(|(_, _, at)| at.x1 - at.x0 > narrower));
        let (_, _, at) = bar(&facts).expect("waited for");
        let wide = at.x1 - at.x0;
        assert_eq!(at.x0, ((1280.0 - wide) / 2.0).floor(), "in the middle");
        widths.push(wide);
    }
    assert_eq!(
        widths[2] - widths[1],
        f64::from(ICON),
        "another window, another icon's width: {widths:?}"
    );

    drop((desk, queue));
    shell.stop_with(session);
}

/// A person's click beside a narrow panel's bar reaches the wallpaper under
/// it, and one on the bar reaches the bar, a menu open or not.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_click_beside_a_narrow_panels_bar_reaches_the_desktop() {
    use perspicax_policy::{Button, Mods};

    let session = Session::start("shell-narrow-beside", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "narrow-beside", &narrow("600", "center"));
    until_bar(&session, rect(340, 0, 600, 40));
    until_colour(&session, (640, 790), BAR);
    assert_eq!(
        colour_at(&session, 100, 790),
        [0x1e, 0x4a, 0x73, 0xff],
        "classic's wallpaper beside the bar"
    );

    let press = |at: (i32, i32), button: Button| {
        session.command(Command::Click {
            at,
            button,
            mods: Mods::default(),
        });
    };
    press((100, 790), Button::Right);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (_, namespace, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-1");
    assert_eq!(opaque[0].x0, 100.0, "the root menu, where the click was");

    // With it open, the start button is still in reach.
    press((360, 780), Button::Left);
    let facts = session
        .wait_for(|facts| menu(facts).is_some_and(|(_, _, _, opaque)| opaque[0].x0 == 340.0));
    let (_, _, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(
        opaque[0].y1,
        800.0 - PANEL_HEIGHT,
        "the start menu, on the bar"
    );

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

/// The menus' surface is drawn over the whole monitor, the shell's panel
/// included, though it leaves the panel out of where it takes clicks. A
/// click there still reaches the panel; to an agent the panel is covered
/// while a menu is open, on policy, because a surface covers everything it
/// draws (#103). #146 would leave the panel undrawn over, and this would
/// change with it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn while_a_menu_is_open_its_panel_is_covered_to_an_agent_though_a_click_reaches_it() {
    use perspicax_index::{Judgement, judge};
    use perspicax_node::Visibility;

    let session = Session::start("shell-panel-covered", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "panel-covered", CLASSIC);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let start = (PANEL_HEIGHT / 2.0, PANEL_HEIGHT / 2.0);
    let button = Rect::new(10.0, 10.0, PANEL_HEIGHT - 10.0, PANEL_HEIGHT - 10.0);
    assert_eq!(
        judge(&session.facts.read(), panel, button).visibility,
        Visibility::Visible,
        "the start button, with no menu open"
    );

    click(&session, panel, start, PointerButton::Left);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (menus, ..) = menu(&facts).expect("waited for");
    assert_eq!(
        judge(&facts, panel, button),
        Judgement {
            visibility: Visibility::Occluded { by: menus },
            unproven: true,
        },
        "covered by the clear part of the menus' surface, which says nothing about it"
    );

    // A click there is not the menus' surface's: it reaches the button, and
    // closes the menu.
    click(&session, panel, start, PointerButton::Left);
    session.wait_for(|facts| menu(facts).is_none());

    shell.stop_with(session);
}

/// The start menu `[shell.start-menu]` writes is what the start button
/// opens: here one item and no search line. One saved while the shell runs
/// is what it opens next.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_start_menu_written_in_the_config_is_what_its_button_opens_and_a_save_changes_it() {
    use std::time::{Duration, Instant};

    let session = Session::start("shell-start-menu", Backend::headless((1280, 800)));
    let marker =
        std::env::temp_dir().join(format!("perspicax-shell-start-menu-{}", std::process::id()));
    std::fs::remove_file(&marker).ok();
    let config = |search: &str| {
        format!(
            "{CLASSIC}[shell.start-menu]\nmode = \"replace\"\nsearch = \"{search}\"\n\
             items = [{{ label = \"Marker\", exec = [\"touch\", {:?}] }}]\n",
            marker.display().to_string()
        )
    };
    let shell = Shell::start(&session, "start-menu", &config("none"));
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let start = (PANEL_HEIGHT / 2.0, PANEL_HEIGHT / 2.0);

    click(&session, panel, start, PointerButton::Left);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (surface, _, _, opaque) = menu(&facts).expect("waited for");
    let one_line = opaque[0].y1 - opaque[0].y0;
    // One line and no search line, so the menu's middle is the line's.
    click(&session, surface, middle(opaque[0]), PointerButton::Left);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the item's program ran");
        thread::sleep(Duration::from_millis(20));
    }
    session.wait_for(|facts| menu(facts).is_none());

    // Saved with a search line: the menu is that much taller once the
    // shell has read it, which it does when the menu next opens.
    shell.rewrite(&config("menu"));
    session.command(Command::ReconfigureShell);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        click(&session, panel, start, PointerButton::Left);
        let facts = session.wait_for(|facts| menu(facts).is_some());
        let (_, _, _, opaque) = menu(&facts).expect("waited for");
        let tall = opaque[0].y1 - opaque[0].y0;
        click(&session, panel, start, PointerButton::Left);
        session.wait_for(|facts| menu(facts).is_none());
        if tall > one_line {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the saved start menu has its search line"
        );
    }

    shell.stop_with(session);
    std::fs::remove_file(marker).ok();
}

/// The start button wears Perspicax's mark, or the icon `[shell.start-menu]`
/// names once it can be read. A file mended after it failed is read on the
/// next save, though the config saved is the same, and a name the icon
/// theme lacks is the mark again.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_start_button_wears_the_mark_or_the_icon_the_config_names() {
    // The mark's inks; and in it, 22 pixels square in the middle of the
    // button at the panel's left end, the middle of its pane and a point on
    // the left side of its run.
    const SKY: [u8; 4] = [0x7b, 0xae, 0xcd, 0xff];
    const TERRACOTTA: [u8; 4] = [0xc0, 0x6a, 0x3c, 0xff];
    const PANE: (usize, usize) = (20, 780);
    const RUN: (usize, usize) = (10, 780);
    const GREEN: [u8; 4] = [0x22, 0x88, 0x44, 0xff];
    const DIM: [u8; 4] = [0x10, 0x20, 0x30, 0xff];

    let session = Session::start("shell-start-icon", Backend::headless((1280, 800)));
    let icon = std::env::temp_dir().join(format!(
        "perspicax-shell-start-icon-{}.svg",
        std::process::id()
    ));
    std::fs::remove_file(&icon).ok();
    let shell = Shell::start(&session, "start-icon", CLASSIC);
    session.wait_for(|facts| panels(facts).len() == 1);
    until_colour(&session, PANE, SKY);
    assert_eq!(colour_at(&session, RUN.0, RUN.1), TERRACOTTA);

    // An icon not there yet, saved with another bar colour: once the bar is
    // that colour, the panel has been drawn since, and the icon looked for.
    let named = format!(
        "{CLASSIC}[theme.palette]\npanel = \"#102030\"\n\
         [shell.start-menu]\nicon = {:?}\n",
        icon.display().to_string()
    );
    shell.rewrite(&named);
    session.command(Command::ReconfigureShell);
    until_colour(&session, (640, 790), DIM);
    assert_eq!(
        colour_at(&session, PANE.0, PANE.1),
        SKY,
        "the mark meanwhile"
    );

    std::fs::write(
        &icon,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#228844"/></svg>"##,
    )
    .expect("an icon");
    shell.rewrite(&named);
    session.command(Command::ReconfigureShell);
    until_colour(&session, PANE, GREEN);
    assert_eq!(colour_at(&session, RUN.0, RUN.1), GREEN, "all of it");

    shell.rewrite(&format!(
        "{CLASSIC}[shell.start-menu]\nicon = \"no-such-icon\"\n"
    ));
    session.command(Command::ReconfigureShell);
    until_colour(&session, PANE, SKY);
    assert_eq!(colour_at(&session, RUN.0, RUN.1), TERRACOTTA);

    shell.stop_with(session);
    std::fs::remove_file(icon).ok();
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_click_on_a_task_brings_its_window_forward() {
    let session = Session::start("shell-task", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "task", CLASSIC);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "first", "first", 0xffff_8000);
    desk.open_coloured(&qh, "second", "second", 0xff00_80ff);
    common::until(&mut queue, &mut desk, |desk| {
        desk.task("first").is_some() && activated(desk, "second")
    });

    // The tasks stand in the order their windows opened, from the start
    // button's right: the second lit, as it has the keyboard. Each is read
    // at its right end, clear of its icon and its title.
    let (first, second) = (PANEL_HEIGHT + TASK / 2.0, PANEL_HEIGHT + TASK * 1.5);
    let face = |middle: f64| ((middle + TASK / 2.0) as usize - 4, 800 - 30);
    until_colour(&session, face(first), FACE);
    until_colour(&session, face(second), LIT);

    // A click on the first brings it forward.
    click(&session, panel, (first, 20.0), PointerButton::Left);
    common::until(&mut queue, &mut desk, |desk| activated(desk, "first"));
    until_colour(&session, face(first), LIT);

    // Another puts it away, since it was forward already.
    click(&session, panel, (first, 20.0), PointerButton::Left);
    common::until(&mut queue, &mut desk, |desk| {
        desk.task("first")
            .is_some_and(|task| task.is(State::Minimized))
    });

    // And a middle click asks the second to close.
    click(&session, panel, (second, 20.0), PointerButton::Middle);
    common::until(&mut queue, &mut desk, |desk| desk.asked_to_close == 1);

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_taskbar_without_titles_gives_each_window_its_icon_and_a_save_brings_titles_back() {
    /// A task showing its icon alone.
    const ICON: f64 = 36.0;

    let session = Session::start("shell-icons", Backend::headless((1280, 800)));
    let config = format!("{CLASSIC}[shell.panel]\ntask-titles = false\n");
    let shell = Shell::start(&session, "icons", &config);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "first", "first", 0xffff_8000);
    desk.open_coloured(&qh, "second", "second", 0xff00_80ff);
    common::until(&mut queue, &mut desk, |desk| {
        desk.task("first").is_some() && activated(desk, "second")
    });

    // Each task an icon's width from the start button's right, read at its
    // right end, clear of its icon; past the second, the bar.
    let face = |end: f64| (end as usize - 4, 800 - 30);
    let (first, second) = (PANEL_HEIGHT + ICON, PANEL_HEIGHT + 2.0 * ICON);
    until_colour(&session, face(first), FACE);
    until_colour(&session, face(second), LIT);
    assert_eq!(
        colour_at(&session, (second + 10.0) as usize, 800 - 30),
        BAR,
        "no more to the taskbar than its icons"
    );

    // A click on the first's icon brings it forward.
    click(
        &session,
        panel,
        (PANEL_HEIGHT + ICON / 2.0, 20.0),
        PointerButton::Left,
    );
    common::until(&mut queue, &mut desk, |desk| activated(desk, "first"));
    until_colour(&session, face(first), LIT);

    // Saved with titles, the same panel widens its tasks again.
    shell.rewrite(CLASSIC);
    session.command(Command::ReconfigureShell);
    until_colour(&session, face(PANEL_HEIGHT + TASK), LIT);
    until_colour(&session, face(PANEL_HEIGHT + 2.0 * TASK), FACE);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    assert_eq!(panels(&facts)[0].1, panel, "redrawn in place");

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_pager_follows_a_switch_by_key() {
    let session = Session::start("shell-pager-key", four_workspaces());
    let shell = Shell::start(&session, "pager-key", PAGER_FIRST);
    session.wait_for(|facts| panels(facts).len() == 1);
    let on_screen = |(x, y): (f64, f64)| (x as usize, 800 - 40 + y as usize);

    // Four cells at the panel's left end, the first workspace's lit.
    until_colour(&session, on_screen(cell(0)), LIT);
    until_colour(&session, on_screen(cell(1)), FACE);
    until_colour(&session, on_screen(cell(3)), FACE);

    // Twice, so that a panel drawn again for some other reason cannot pass
    // for one that follows.
    session.perform(Action::GoToWorkspace(2));
    until_colour(&session, on_screen(cell(1)), LIT);
    until_colour(&session, on_screen(cell(0)), FACE);
    session.perform(Action::GoToWorkspace(4));
    until_colour(&session, on_screen(cell(3)), LIT);
    until_colour(&session, on_screen(cell(1)), FACE);

    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_click_on_a_workspace_switches_to_it() {
    let session = Session::start("shell-pager-click", four_workspaces());
    let shell = Shell::start(&session, "pager-click", PAGER_FIRST);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pager(&globals, &qh);
    common::until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["1"]
    });
    // Drawn, so there are cells to click.
    until_colour(&session, (cell(0).0 as usize, 770), LIT);

    click(&session, panel, cell(2), PointerButton::Left);
    common::until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["3"]
    });

    drop((desk, queue));
    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_the_taskbar_rule_off_the_panel_still_runs() {
    let session = Session::start("shell-taskbar-off", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "taskbar-off", CLASSIC);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    desk.open_coloured(&qh, "orange", "orange", 0xffff_8000);
    common::until(&mut queue, &mut desk, |desk| activated(desk, "orange"));
    #[cfg(feature = "capture")]
    let task = ((PANEL_HEIGHT + TASK) as usize - 4, 800 - 30);
    #[cfg(feature = "capture")]
    until_colour(&session, task, LIT);

    // The taskbar's and the pager's protocols taken from every program, the
    // shell among them, as a person narrowing `[protocols]` does.
    session.command(Command::Protocols(
        Access::open()
            .with(Protocol::ForeignToplevelManagement, Rule::Off)
            .with(Protocol::Workspace, Rule::Off),
    ));
    common::until(&mut queue, &mut desk, |desk| {
        desk.taskbar_finished && desk.pager_finished
    });

    // The window is gone from the panel, and the panel is still there: its
    // start button still opens the start menu.
    #[cfg(feature = "capture")]
    until_colour(&session, task, BAR);
    click(
        &session,
        panel,
        (PANEL_HEIGHT / 2.0, PANEL_HEIGHT / 2.0),
        PointerButton::Left,
    );
    session.wait_for(|facts| menu(facts).is_some());

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_taskbar_and_pager_taken_away_come_back_with_their_rules() {
    let session = Session::start("shell-taskbar-back", four_workspaces());
    // The taskbar from the left end, and the pager after it at the right.
    let config = "profile = \"classic\"\n[shell.panel]\nitems = [\"taskbar\", \"pager\"]\n";
    let shell = Shell::start(&session, "taskbar-back", config);
    session.wait_for(|facts| panels(facts).len() == 1);
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_coloured(&qh, "orange", "orange", 0xffff_8000);
    common::until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let task = (TASK as usize - 4, 800 - 30);
    let first_cell = (1280 - 218 + 4 + 4, 800 - 30);
    until_colour(&session, task, LIT);
    until_colour(&session, first_cell, LIT);

    let off = Access::open()
        .with(Protocol::ForeignToplevelManagement, Rule::Off)
        .with(Protocol::Workspace, Rule::Off);
    session.command(Command::Protocols(off));
    until_colour(&session, task, BAR);
    until_colour(&session, first_cell, BAR);

    // Given back: the shell takes both back, with no restart.
    session.command(Command::Protocols(Access::open()));
    until_colour(&session, task, LIT);
    until_colour(&session, first_cell, LIT);

    drop((desk, queue));
    shell.stop_with(session);
}

/// A panel of the layout indicator and the clock alone, so the indicator is
/// at its left end whatever the font makes its width.
const LAYOUT_FIRST: &str =
    "profile = \"classic\"\n[shell.panel]\nitems = [\"layout\", \"clock\"]\n";

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_click_on_the_layout_indicator_moves_to_the_next_layout() {
    let session = Session::start("shell-layout", Backend::headless((1280, 800)));
    session.command(Command::Keymap(Keymap {
        layout: "us,ru".to_owned(),
        ..Keymap::default()
    }));
    let shell = Shell::start(&session, "layout", LAYOUT_FIRST);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    // A second listener on the channel, to hear which layout is in use.
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_shell(&globals, &qh);
    common::until(&mut queue, &mut desk, |desk| {
        desk.layouts.len() == 2 && desk.active_layout == Some(0)
    });

    // Inside the padding before "US", at the panel's left end.
    let indicator = (6.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, indicator, PointerButton::Left);
    common::until(&mut queue, &mut desk, |desk| desk.active_layout == Some(1));
    click(&session, panel, indicator, PointerButton::Left);
    common::until(&mut queue, &mut desk, |desk| desk.active_layout == Some(0));

    drop((desk, queue));
    shell.stop_with(session);
}

/// A panel along the bottom holding the start button, the taskbar and the
/// tray, the tray last: its first icon in the 30 pixels at the panel's
/// right end.
#[cfg(feature = "capture")]
const TRAY_LAST: &str =
    "profile = \"classic\"\n[shell.panel]\nitems = [\"start\", \"taskbar\", \"tray\"]\n";

/// The middle of the tray's first icon, on a 1280 by 800 monitor.
#[cfg(feature = "capture")]
const STATUS: (f64, f64) = (1280.0 - 15.0, 20.0);

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_item_on_the_bus_appears_in_the_tray() {
    let session = Session::start("shell-tray", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "tray", TRAY_LAST);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let at = (STATUS.0 as usize, 800 - 20);
    until_colour(&session, at, BAR);

    // A program registers its icon, a picture of magenta.
    let notifier = notifier::Notifier::register(&shell.bus.address);
    until_colour(&session, at, notifier::MAGENTA);

    // A click on it is asked of the program.
    click(&session, panel, STATUS, PointerButton::Left);
    notifier.until_asked("Activate");

    // The program leaves the bus, and its icon goes with it.
    notifier.leave();
    until_colour(&session, at, BAR);

    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tray_menu_choice_is_sent_back() {
    let session = Session::start("shell-tray-menu", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "tray-menu", TRAY_LAST);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let notifier = notifier::Notifier::register(&shell.bus.address);
    until_colour(&session, (STATUS.0 as usize, 800 - 20), notifier::MAGENTA);

    // A right click opens the program's menu, above its icon.
    click(&session, panel, STATUS, PointerButton::Right);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (surface, namespace, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-1");
    assert_eq!(opaque[0].y1, 800.0 - PANEL_HEIGHT, "standing on the panel");
    assert_eq!(opaque[0].x1, 1280.0, "and kept on the monitor");

    // It holds one item, and choosing it is told to the program.
    click(&session, surface, middle(opaque[0]), PointerButton::Left);
    notifier.until_asked("clicked 1");
    session.wait_for(|facts| menu(facts).is_none());

    notifier.leave();
    shell.stop_with(session);
}

/// A panel of the mixer alone, its speaker at the panel's right end.
#[cfg(feature = "capture")]
const MIXER_ONLY: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n";

/// The panel's ink, and its faint ink.
#[cfg(feature = "capture")]
const INK: [u8; 4] = [0xfc, 0xfc, 0xfc, 0xff];
#[cfg(feature = "capture")]
const FAINT: [u8; 4] = [0x9a, 0xa0, 0xa6, 0xff];

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_click_on_the_speaker_mutes_the_sound_servers_output_and_a_server_gone_is_faint() {
    use std::time::{Duration, Instant};

    let sound = SoundServer::start("mute");
    let session = Session::start("shell-mute", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "mute",
        MIXER_ONLY,
        Some(sound.socket()),
    );
    sound.pactl(&["set-sink-volume", "one", "100%"]);
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    // The speaker, in the 30 pixels at the panel's right end: its box, in
    // the ink once the shell has heard the server; and where its third wave
    // crosses the middle, in the ink while the sound is loud and on, and the
    // bar's colour while it is muted.
    let speaker = (1258, 800 - 20);
    let wave = (1273, 800 - 20);
    until_colour(&session, speaker, INK);
    until_colour(&session, wave, INK);

    let muted = |yes: bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        let want = if yes { "Mute: yes" } else { "Mute: no" };
        while sound.pactl(&["get-sink-mute", "one"]).trim() != want {
            assert!(Instant::now() < deadline, "the output never said {want}");
            thread::sleep(Duration::from_millis(50));
        }
    };
    let middle = (1280.0 - 15.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, middle, PointerButton::Left);
    muted(true);
    until_colour(&session, wave, BAR);
    // Brought back elsewhere, as by a key bound to wpctl, it is heard: the
    // next click mutes it again, rather than bringing back what is on.
    sound.pactl(&["set-sink-mute", "one", "0"]);
    until_colour(&session, wave, INK);
    click(&session, panel, middle, PointerButton::Left);
    muted(true);
    until_colour(&session, wave, BAR);
    click(&session, panel, middle, PointerButton::Left);
    muted(false);

    // The server goes: the speaker is faint, with no one to ask.
    drop(sound);
    until_colour(&session, speaker, FAINT);
    shell.stop_with(session);
}

/// A panel of the mixer alone, its volume then its speaker, the speaker at
/// the panel's right end.
#[cfg(feature = "capture")]
const VOLUME_AND_MUTE: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n\
    [shell.mixer]\npanel = [\"volume\", \"mute\"]\n";

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn the_wheel_over_the_mixer_steps_the_volume_to_the_limit_and_a_middle_click_mutes() {
    use std::time::{Duration, Instant};

    use perspicax_policy::Mods;

    let sound = SoundServer::start("wheel");
    sound.pactl(&["set-sink-volume", "one", "24%"]);
    let session = Session::start("shell-wheel", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "wheel",
        VOLUME_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let (speaker, wave) = ((1258, 800 - 20), (1273, 800 - 20));
    until_colour(&session, speaker, INK);

    let volume = || {
        let said = sound.pactl(&["get-sink-volume", "one"]);
        said.split('/')
            .nth(1)
            .and_then(|percent| percent.trim().trim_end_matches('%').parse::<u32>().ok())
            .unwrap_or_else(|| panic!("a volume in {said:?}"))
    };
    let until_volume = |want: u32| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while volume() != want {
            assert!(
                Instant::now() < deadline,
                "the volume never reached {want}%"
            );
            thread::sleep(Duration::from_millis(50));
        }
    };
    // Over the volume, left of the speaker.
    let turn = |notches: i32| {
        session.command(Command::Scroll {
            at: (1240, 800 - 20),
            v120: (0, notches * 120),
            mods: Mods::default(),
        });
    };
    turn(-1);
    until_volume(25);
    // Two notches before the server answers the first go two steps.
    turn(-1);
    turn(-1);
    until_volume(35);
    turn(1);
    until_volume(30);

    // Near the top, the next notch stops at the limit, and the one after
    // asks nothing more.
    sound.pactl(&["set-sink-volume", "one", "98%"]);
    until_colour(&session, wave, INK);
    turn(-1);
    until_volume(100);
    turn(-1);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(volume(), 100);

    // A middle click anywhere on it mutes, and brings the sound back.
    let middle = (1240.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, middle, PointerButton::Middle);
    until_colour(&session, wave, BAR);
    assert_eq!(sound.pactl(&["get-sink-mute", "one"]).trim(), "Mute: yes");
    click(&session, panel, middle, PointerButton::Middle);
    until_colour(&session, wave, INK);

    drop(sound);
    shell.stop_with(session);
}

/// A panel of the mixer alone, its output then its speaker, the speaker at
/// the panel's right end.
#[cfg(feature = "capture")]
const OUTPUT_AND_MUTE: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n\
    [shell.mixer]\npanel = [\"output\", \"mute\"]\n";

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_click_on_the_output_switches_to_the_next_and_round() {
    use std::time::{Duration, Instant};

    let sound = SoundServer::start("output");
    let session = Session::start("shell-output", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "output",
        OUTPUT_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let default = || sound.pactl(&["get-default-sink"]).trim().to_owned();
    let until_default = |want: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while default() != want {
            assert!(Instant::now() < deadline, "the default never became {want}");
            thread::sleep(Duration::from_millis(50));
        }
    };
    // The speaker's box, at the right end, in the ink once the shell has
    // heard the server; and the output's room after its name, left of it.
    until_colour(&session, (1258, 800 - 20), INK);
    let output = (1280.0 - 30.0 - 5.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, output, PointerButton::Left);
    until_default("two");
    click(&session, panel, output, PointerButton::Left);
    until_default("one");

    drop(sound);
    shell.stop_with(session);
}

/// A panel of the mixer alone, its moving bars then its speaker, the
/// speaker at the panel's right end.
#[cfg(feature = "capture")]
const LEVELS_AND_MUTE: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n\
    [shell.mixer]\npanel = [\"levels\", \"mute\"]\n";

/// The theme's accent, which the moving bars are drawn in.
#[cfg(feature = "capture")]
const ACCENT: [u8; 4] = [0x3d, 0xae, 0xe9, 0xff];

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn the_bars_rise_with_what_plays_and_fall_once_it_stops() {
    use std::{io::Write as _, time::Duration};

    let sound = SoundServer::start("levels");
    sound.pactl(&["set-sink-volume", "one", "100%"]);
    let session = Session::start("shell-levels", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "levels",
        LEVELS_AND_MUTE,
        Some(sound.socket()),
    );
    until_colour(&session, (1258, 800 - 20), INK);
    // The bar of the band holding 1 kHz, the ninth of sixteen, low in the
    // slot left of the speaker: 75 wide, its bars 6 in, each 3 and a gap.
    let bar = (1280 - 30 - 75 + 6 + 8 * 4 + 1, 800 - 10);
    assert_eq!(
        colour_at(&session, bar.0, bar.1),
        BAR,
        "still with nothing playing"
    );

    // A loud 1 kHz sine, played into the default output.
    let mut player = sound
        .command("pacat")
        .args([
            "--raw",
            "--format=float32le",
            "--rate=22050",
            "--channels=1",
        ])
        .args(["--device=one"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("pacat, which a desktop with sound has");
    let mut feed = player.stdin.take().expect("its input");
    let playing = thread::spawn(move || {
        let second: Vec<u8> = (0..22_050)
            .flat_map(|at: u32| {
                let phase = 2.0 * std::f32::consts::PI * 1000.0 * at as f32 / 22_050.0;
                (0.5 * phase.sin()).to_le_bytes()
            })
            .collect();
        for _ in 0..20 {
            if feed.write_all(&second).is_err() {
                return;
            }
        }
    });
    until_colour(&session, bar, ACCENT);

    // Muted as they move, the whole panel is drawn again, and each picture
    // after shows the cross where the third wave was: none goes back to a
    // buffer of before.
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    let wave = (1273, 800 - 20);
    until_colour(&session, wave, INK);
    click(
        &session,
        panel,
        (1280.0 - 15.0, PANEL_HEIGHT / 2.0),
        PointerButton::Left,
    );
    until_colour(&session, wave, BAR);
    for _ in 0..20 {
        assert_eq!(
            colour_at(&session, wave.0, wave.1),
            BAR,
            "a picture of before came back"
        );
        thread::sleep(Duration::from_millis(25));
    }

    player.kill().ok();
    player.wait().ok();
    playing.join().ok();
    until_colour(&session, bar, BAR);

    drop(sound);
    shell.stop_with(session);
}

/// A program's status icon, served as a program serves one: a title, a
/// picture of magenta, and a menu of one item, Quit. What it is asked is
/// kept, to be waited for.
#[cfg(feature = "capture")]
mod notifier {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    /// Its picture's colour, opaque.
    pub(super) const MAGENTA: [u8; 4] = [0xff, 0x00, 0xff, 0xff];

    /// What it was asked, in order.
    type Asked = Arc<Mutex<Vec<String>>>;

    pub(super) struct Notifier {
        runtime: tokio::runtime::Runtime,
        connection: zbus::Connection,
        asked: Asked,
    }

    struct Item {
        asked: Asked,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl Item {
        fn activate(&self, _x: i32, _y: i32) {
            self.asked.lock().unwrap().push("Activate".to_owned());
        }

        fn secondary_activate(&self, _x: i32, _y: i32) {
            self.asked
                .lock()
                .unwrap()
                .push("SecondaryActivate".to_owned());
        }

        fn context_menu(&self, _x: i32, _y: i32) {
            self.asked.lock().unwrap().push("ContextMenu".to_owned());
        }

        #[zbus(property)]
        fn id(&self) -> &str {
            "perspicax-test"
        }

        #[zbus(property)]
        fn title(&self) -> &str {
            "Test icon"
        }

        #[zbus(property)]
        fn status(&self) -> &str {
            "Active"
        }

        #[zbus(property)]
        fn icon_name(&self) -> &str {
            ""
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
            // Alpha, red, green, blue, each pixel.
            let argb = [0xff, MAGENTA[0], MAGENTA[1], MAGENTA[2]];
            vec![(22, 22, argb.repeat(22 * 22))]
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn menu(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from("/MenuBar").unwrap()
        }
    }

    struct Menu {
        asked: Asked,
    }

    /// An item of a dbusmenu layout: `(ia{sv}av)`.
    type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

    fn said(key: &str, value: &str) -> (String, OwnedValue) {
        (
            key.to_owned(),
            OwnedValue::try_from(Value::from(value)).unwrap(),
        )
    }

    #[zbus::interface(name = "com.canonical.dbusmenu")]
    impl Menu {
        fn get_layout(&self, _parent: i32, _depth: i32, _properties: Vec<String>) -> (u32, Layout) {
            let quit: Layout = (1, HashMap::from([said("label", "_Quit")]), Vec::new());
            let root: Layout = (
                0,
                HashMap::from([said("children-display", "submenu")]),
                vec![OwnedValue::try_from(Value::from(quit)).unwrap()],
            );
            (1, root)
        }

        fn about_to_show(&self, _id: i32) -> bool {
            false
        }

        fn event(&self, id: i32, event: &str, _data: OwnedValue, _timestamp: u32) {
            self.asked.lock().unwrap().push(format!("{event} {id}"));
        }

        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }
    }

    impl Notifier {
        /// Serve the icon on the bus at `address`, and register it with the
        /// watcher there once there is one.
        pub(super) fn register(address: &str) -> Self {
            let runtime = tokio::runtime::Runtime::new().expect("a runtime");
            let asked = Asked::default();
            let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
            let connection = runtime
                .block_on(async {
                    zbus::connection::Builder::address(address)?
                        .name(name.as_str())?
                        .serve_at(
                            "/StatusNotifierItem",
                            Item {
                                asked: asked.clone(),
                            },
                        )?
                        .serve_at(
                            "/MenuBar",
                            Menu {
                                asked: asked.clone(),
                            },
                        )?
                        .build()
                        .await
                })
                .expect("the icon served");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let registered = runtime.block_on(connection.call_method(
                    Some("org.kde.StatusNotifierWatcher"),
                    "/StatusNotifierWatcher",
                    Some("org.kde.StatusNotifierWatcher"),
                    "RegisterStatusNotifierItem",
                    &name,
                ));
                match registered {
                    Ok(_) => break,
                    Err(error) => assert!(
                        Instant::now() < deadline,
                        "the shell's watcher never took the registration: {error}"
                    ),
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Self {
                runtime,
                connection,
                asked,
            }
        }

        /// Wait until the icon has been asked `what`.
        pub(super) fn until_asked(&self, what: &str) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.asked.lock().unwrap().iter().any(|asked| asked == what) {
                assert!(
                    Instant::now() < deadline,
                    "never asked {what}; asked {:?}",
                    self.asked.lock().unwrap()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// Leave the bus, as a program that quits does.
        pub(super) fn leave(self) {
            let Self {
                runtime,
                connection,
                ..
            } = self;
            runtime.block_on(async move { drop(connection) });
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }
}
