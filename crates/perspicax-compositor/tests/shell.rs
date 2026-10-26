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
//! Told perspicax refused a keymap, it says why on the `overlay` layer, in
//! the middle above the foot of the screen, for a few seconds, and a second
//! refusal takes the first's place; so it does of a save it cannot use
//! itself.
//! A workspace with a wallpaper of its own
//! shows it while it is the one showing, on whichever monitor shows it.
//! A right-click on the wallpaper, or the root
//! menu's key, opens a menu on the `overlay` layer where the pointer is, and
//! choosing an item in it runs the item's program. In the classic profile a
//! panel along the bottom of each monitor keeps windows above it, and its
//! start button, or the start menu's key, opens the start menu standing on
//! it. The start menu holds, and its button wears, what `[shell.start-menu]`
//! says, and a save changes them while the shell runs. Its taskbar lists the windows, and a click on one brings it forward,
//! puts it away or closes it, and a right-click opens its window's menu
//! beside it, whose choice is made of the window; its pager shows the workspaces, follows a
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
use perspicax_index::{Action as Verb, HostFacts, Layer, PointerButton, Refusal, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{Access, Action, Ended, Place, Protocol, Rule, Shape, Side};
use wayland_client::Connection;
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

/// The shell, running on a thread against a session.
struct Shell {
    thread: thread::JoinHandle<Result<(), perspicax_shell::Error>>,
    config: PathBuf,
    /// The file its mixer keeps what is hidden and renamed in: one of the
    /// test's own.
    #[cfg_attr(
        not(feature = "capture"),
        expect(dead_code, reason = "the mixer's tests'")
    )]
    state: PathBuf,
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
        // Never the person's own.
        let state = std::env::temp_dir().join(format!(
            "perspicax-shell-{name}-{}-mixer.json",
            std::process::id()
        ));
        std::fs::remove_file(&state).ok();
        let options = perspicax_shell::Options {
            connection: Some(connection),
            config: Some(path.clone()),
            session_bus: Some(bus.address.clone()),
            sound_server: Some(sound.unwrap_or(nowhere)),
            state: Some(state.clone()),
        };
        Self {
            thread: thread::spawn(move || perspicax_shell::run(options)),
            config: path,
            state,
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
            } if surface.mapped && namespace.starts_with("perspicax-menu-") => Some((
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

/// Close the open start menu as a click away from it does: on the clear
/// part of the menus' surface, `menus`, at the far end of the monitor from
/// the start button, which is under that surface too.
fn click_off_the_menu(session: &Session, menus: SurfaceId) {
    click(session, menus, (1200.0, 100.0), PointerButton::Left);
    session.wait_for(|facts| menu(facts).is_none());
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
    // The pointer on the wallpaper, where the windows will open over it.
    click(&session, desktop(&facts), (20.0, 20.0), PointerButton::Left);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "first", "slack");
    desk.open_window(&qh, "second", "slack");
    common::until(&mut queue, &mut desk, |desk| {
        desk.task("first").is_some() && activated(desk, "second")
    });

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
    let (menus, _, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(
        (opaque[0].x0, opaque[0].y1),
        (340.0, 800.0 - PANEL_HEIGHT),
        "above the button, where the bar put it"
    );
    click_off_the_menu(&session, menus);

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
    let (menus, namespace, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-1");
    assert_eq!(
        (opaque[0].x0, opaque[0].y1),
        (0.0, 800.0 - PANEL_HEIGHT),
        "above the button"
    );

    // A click away from it closes it.
    click_off_the_menu(&session, menus);

    shell.stop_with(session);
}

/// The menus' surface is drawn over the whole monitor, the shell's panel
/// included, though it leaves the panel out of where it takes clicks. To an
/// agent the panel is covered while a menu is open, on policy, because a
/// surface covers everything it draws (#103), and an agent's click there is
/// refused as it is sent, naming the menus' surface (#158). #146 would
/// leave the panel undrawn over, and this would change with it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn while_a_menu_is_open_its_panel_is_covered_to_an_agent_and_its_click_refused() {
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

    // Judged again as it is sent, and refused as the gate would refuse it:
    // nothing is sent, and the menu stays open.
    assert_eq!(
        Host::new(&session.facts, &session.requests)
            .act(
                panel,
                &Verb::Click {
                    at: button,
                    button: PointerButton::Left,
                },
            )
            .map(|_| ()),
        Err(ActError::OutOfSight(Refusal::Occluded { by: menus }))
    );
    assert!(menu(&session.facts.read()).is_some(), "still open");
    click_off_the_menu(&session, menus);

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
        let (menus, _, _, opaque) = menu(&facts).expect("waited for");
        let tall = opaque[0].y1 - opaque[0].y0;
        click_off_the_menu(&session, menus);
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

/// A person's right click on a task opens its window's menu beside it, and
/// what is chosen there is made of the window. A second right click on the
/// task only closes the menu, and the menu goes with its window. The clicks
/// are a person's, since an agent's on a panel a menu covers is refused.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_right_click_on_a_task_opens_its_windows_menu_whose_choice_is_made_of_it() {
    use perspicax_policy::{Button, Mods};
    use perspicax_protocols::shell::v1::client::perspicax_shell_v1::WindowState;

    let session = Session::start("shell-window-menu", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "window-menu", CLASSIC);
    session.wait_for(|facts| panels(facts).len() == 1);
    // A client of the test's own, told what is made of each window as the
    // shell is.
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    desk.bind_shell(&globals, &qh);
    desk.open_framed(&qh, "framed");
    common::until(&mut queue, &mut desk, |desk| {
        desk.told_of("framed").0.is_some()
    });
    let task = (PANEL_HEIGHT + TASK / 2.0) as i32;
    until_colour(
        &session,
        ((task + TASK as i32 / 2 - 4) as usize, 800 - 30),
        LIT,
    );
    let press = |at: (i32, i32), button: Button| {
        session.command(Command::Click {
            at,
            button,
            mods: Mods::default(),
        });
    };

    press((task, 780), Button::Right);
    let facts = session.wait_for(|facts| menu(facts).is_some());
    let (_, namespace, _, opaque) = menu(&facts).expect("waited for");
    assert_eq!(namespace, "perspicax-menu-HEADLESS-1");
    assert_eq!(opaque[0].y1, 800.0 - PANEL_HEIGHT, "standing on the panel");
    assert_eq!(opaque[0].x0, PANEL_HEIGHT, "at its task's left end");

    // Its first line, 5 below its top and 30 high, is Sticky.
    let sticky = (middle(opaque[0]).0 as i32, opaque[0].y0 as i32 + 20);
    press(sticky, Button::Left);
    common::until(&mut queue, &mut desk, |desk| {
        desk.told_of("framed")
            .0
            .is_some_and(|told| told.state.contains(WindowState::Sticky))
    });
    session.wait_for(|facts| menu(facts).is_none());

    press((task, 780), Button::Right);
    session.wait_for(|facts| menu(facts).is_some());
    press((task, 780), Button::Right);
    session.wait_for(|facts| menu(facts).is_none());
    // Opened again, it would be up within a turn or two of either loop.
    thread::sleep(std::time::Duration::from_millis(500));
    assert!(menu(&session.facts.read()).is_none(), "and it stays shut");

    press((task, 780), Button::Right);
    session.wait_for(|facts| menu(facts).is_some());
    desk.windows.remove(0);
    common::until(&mut queue, &mut desk, |desk| desk.task("framed").is_none());
    session.wait_for(|facts| menu(facts).is_none());

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

/// A panel's tooltip, if one is up and drawn on: its id, its namespace, and
/// where it is.
#[cfg(feature = "capture")]
fn tooltip(facts: &HostFacts) -> Option<(SurfaceId, String, Rect)> {
    facts
        .surfaces()
        .iter()
        .find_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Overlay,
                namespace,
            } if surface.mapped && namespace.starts_with("perspicax-tooltip-") => {
                Some((surface.id, namespace.clone(), surface.geometry))
            }
            _ => None,
        })
}

/// A task showing its icon alone, and how tall a tooltip is and how far it
/// stands off the bar.
#[cfg(feature = "capture")]
const ICON: f64 = 36.0;
#[cfg(feature = "capture")]
const TIP: f64 = 28.0;
#[cfg(feature = "capture")]
const TIP_GAP: f64 = 4.0;

/// A session named `name` and the shell `shell` on it, the classic panel
/// with `panel` written in its `[shell.panel]` and its tasks icons alone,
/// listing two windows, "first" and "second", the second with the keyboard;
/// and the client the windows are of. The panel has drawn them, as read
/// `row` pixels down the monitor.
#[cfg(feature = "capture")]
fn two_icons(
    (name, shell): (&str, &str),
    panel: &str,
    row: usize,
) -> (Session, Shell, Desk, wayland_client::EventQueue<Desk>) {
    let session = Session::start(name, Backend::headless((1280, 800)));
    let config = format!("{CLASSIC}[shell.panel]\ntask-titles = false\n{panel}");
    let shell = Shell::start(&session, shell, &config);
    session.wait_for(|facts| panels(facts).len() == 1);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_coloured(&qh, "first", "first", 0xffff_8000);
    desk.open_coloured(&qh, "second", "second", 0xff00_80ff);
    common::until(&mut queue, &mut desk, |desk| {
        desk.task("first").is_some() && activated(desk, "second")
    });
    until_colour(
        &session,
        ((PANEL_HEIGHT + 2.0 * ICON) as usize - 4, row),
        LIT,
    );
    (session, shell, desk, queue)
}

/// The middle of `rect`, across.
#[cfg(feature = "capture")]
fn across(rect: Rect) -> f64 {
    (rect.x0 + rect.x1) / 2.0
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn resting_on_a_task_shows_its_title_above_the_bar_and_moving_on_moves_it_at_once() {
    use std::time::{Duration, Instant};

    let (session, shell, desk, queue) = two_icons(("shell-tip", "tip"), "", 800 - 30);

    // From the origin, where the pointer starts, to rest on the first task.
    let first = PANEL_HEIGHT + ICON / 2.0;
    let moved = Instant::now();
    session.command(Command::Motion {
        by: (first as i32, 780),
    });
    let facts = session.wait_for(|facts| tooltip(facts).is_some());
    assert!(
        moved.elapsed() >= Duration::from_millis(600),
        "shown before the pointer rested: {:?}",
        moved.elapsed()
    );
    let (tip, namespace, rect) = tooltip(&facts).expect("a tip");
    assert_eq!(namespace, "perspicax-tooltip-HEADLESS-1");
    let foot = 800.0 - PANEL_HEIGHT - TIP_GAP;
    assert_eq!(
        (rect.y0, rect.y1),
        (foot - TIP, foot),
        "standing off the bar"
    );
    assert!(
        (across(rect) - first).abs() <= 1.0,
        "over its task: {rect:?}"
    );

    // Onto the second: the same tip, moved there, never taken away to wait
    // again.
    session.command(Command::Motion {
        by: (ICON as i32, 0),
    });
    let facts = session.wait_for(|facts| {
        tooltip(facts).is_some_and(|(_, _, rect)| (across(rect) - (first + ICON)).abs() <= 1.0)
    });
    assert_eq!(tooltip(&facts).expect("a tip").0, tip, "the same surface");

    // Off the bar, it goes.
    session.command(Command::Motion { by: (0, -100) });
    session.wait_for(|facts| tooltip(facts).is_none());

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_press_or_a_menu_takes_a_tooltip_away() {
    use std::time::Duration;

    use perspicax_policy::{Button, Mods};

    let (session, shell, mut desk, mut queue) =
        two_icons(("shell-tip-press", "tip-press"), "", 800 - 30);
    let first = PANEL_HEIGHT + ICON / 2.0;
    session.command(Command::Motion {
        by: (first as i32, 780),
    });
    session.wait_for(|facts| tooltip(facts).is_some());

    // A press on the task takes its tip away, and while the pointer rests
    // there it stays away.
    session.command(Command::Click {
        at: (first as i32, 780),
        button: Button::Left,
        mods: Mods::default(),
    });
    common::until(&mut queue, &mut desk, |desk| activated(desk, "first"));
    session.wait_for(|facts| tooltip(facts).is_none());
    thread::sleep(Duration::from_secs(1));
    assert!(
        tooltip(&session.wait_for(|_| true)).is_none(),
        "the task pressed shows none"
    );

    // The other's shows; the start menu opening takes it away.
    session.command(Command::Motion {
        by: (ICON as i32, 0),
    });
    session.wait_for(|facts| tooltip(facts).is_some());
    session.perform(Action::StartMenu);
    session.wait_for(|facts| menu(facts).is_some() && tooltip(facts).is_none());

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_tooltip_follows_its_windows_title_and_the_task_that_slides_under_a_still_pointer() {
    const LONGER: &str = "first, and now a title a good deal longer";

    let (session, shell, mut desk, mut queue) =
        two_icons(("shell-tip-follows", "tip-follows"), "", 800 - 30);
    let wide = |rect: Rect| rect.x1 - rect.x0;
    session.command(Command::Motion {
        by: ((PANEL_HEIGHT + ICON / 2.0) as i32, 780),
    });
    let facts = session.wait_for(|facts| tooltip(facts).is_some());
    let (tip, _, first) = tooltip(&facts).expect("a tip");

    // The window retitled, its tip says so where it stands, on the same
    // surface.
    desk.windows[0].set_title(LONGER);
    common::until(&mut queue, &mut desk, |desk| desk.task(LONGER).is_some());
    let facts = session.wait_for(|facts| {
        tooltip(facts).is_some_and(|(_, _, rect)| wide(rect) > wide(first) + 40.0)
    });
    let (retitled, _, longer) = tooltip(&facts).expect("a tip");
    assert_eq!(retitled, tip, "the same surface");

    // Closed, the second's task slides under the pointer, and the tip goes
    // with it.
    drop(desk.windows.remove(0));
    common::until(&mut queue, &mut desk, |desk| desk.task(LONGER).is_none());
    let facts = session.wait_for(|facts| {
        tooltip(facts).is_some_and(|(_, _, rect)| wide(rect) < wide(longer) - 40.0)
    });
    assert_eq!(tooltip(&facts).expect("a tip").0, tip, "the same surface");

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_tooltips_off_resting_on_a_task_shows_none_and_a_save_turns_them_on_and_off() {
    use std::time::Duration;

    let (session, shell, desk, queue) =
        two_icons(("shell-tip-off", "tip-off"), "tooltips = false\n", 800 - 30);
    session.command(Command::Motion {
        by: ((PANEL_HEIGHT + ICON / 2.0) as i32, 780),
    });
    thread::sleep(Duration::from_millis(1500));
    assert!(
        tooltip(&session.wait_for(|_| true)).is_none(),
        "none, rested well past the wait"
    );

    // Saved on, the next move shows one.
    let icons = format!("{CLASSIC}[shell.panel]\ntask-titles = false\n");
    shell.rewrite(&icons);
    session.command(Command::ReconfigureShell);
    let tip = || {
        session.command(Command::Motion { by: (1, 0) });
        session.wait_for(|facts| tooltip(facts).is_some());
    };
    tip();

    // Saved off, the one shown goes at once, the pointer still.
    shell.rewrite(&format!("{icons}tooltips = false\n"));
    session.command(Command::ReconfigureShell);
    session.wait_for(|facts| tooltip(facts).is_none());

    drop((desk, queue));
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_top_panels_tooltip_hangs_below_it() {
    let (session, shell, desk, queue) =
        two_icons(("shell-tip-top", "tip-top"), "edge = \"top\"\n", 10);
    session.command(Command::Motion {
        by: ((PANEL_HEIGHT + ICON / 2.0) as i32, 20),
    });
    let facts = session.wait_for(|facts| tooltip(facts).is_some());
    let (_, _, rect) = tooltip(&facts).expect("a tip");
    let top = PANEL_HEIGHT + TIP_GAP;
    assert_eq!((rect.y0, rect.y1), (top, top + TIP), "hanging off the bar");

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
fn resting_on_a_status_icon_shows_its_programs_title_kept_on_the_monitor() {
    let session = Session::start("shell-tray-tip", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "tray-tip", TRAY_LAST);
    session.wait_for(|facts| panels(facts).len() == 1);
    let notifier = notifier::Notifier::register(&shell.bus.address);
    until_colour(&session, (STATUS.0 as usize, 800 - 20), notifier::MAGENTA);

    // From the origin, where the pointer starts, to rest on the icon at the
    // panel's right end: its tip stands off the bar, short of the monitor's
    // side.
    session.command(Command::Motion {
        by: (STATUS.0 as i32, 800 - 20),
    });
    let facts = session.wait_for(|facts| tooltip(facts).is_some());
    let (_, namespace, rect) = tooltip(&facts).expect("a tip");
    assert_eq!(namespace, "perspicax-tooltip-HEADLESS-1");
    assert_eq!(rect.y1, 800.0 - PANEL_HEIGHT - TIP_GAP, "off the bar");
    assert!(rect.x1 <= 1280.0 - 4.0, "kept on the monitor: {rect:?}");
    assert!(
        rect.x0 < STATUS.0 && STATUS.0 < rect.x1,
        "over its icon: {rect:?}"
    );

    notifier.leave();
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

/// A panel of the mixer's speaker alone, at the panel's right end.
#[cfg(feature = "capture")]
const MIXER_ONLY: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n\
    [shell.mixer]\npanel = [\"mute\"]\n";

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

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn an_output_named_and_hidden_in_the_mixer_is_kept_for_the_person_alone_and_never_switched_to() {
    use std::{os::unix::fs::PermissionsExt as _, time::Duration};

    let sound = SoundServer::start("hide");
    let session = Session::start("shell-hide", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "hide",
        OUTPUT_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    until_colour(&session, (1258, 800 - 20), INK);
    click(
        &session,
        panel,
        (1280.0 - 15.0, PANEL_HEIGHT / 2.0),
        PointerButton::Right,
    );
    let facts = session.wait_for(|facts| mixer(facts).is_some());
    let (surface, rect) = mixer(&facts).expect("waited for");
    // "two" is the output's second row: under 1 of edge and 12 of room, a
    // 22-pixel title, a 32-pixel row, and "one", 28; the button at its end
    // is 28 square, against the room on the right.
    let two = 13.0 + 22.0 + 32.0 + 28.0;
    click(
        &session,
        surface,
        (rect.x1 - 13.0 - 14.0, rect.y0 + two + 14.0),
        PointerButton::Left,
    );
    // Unfolded under it: its key, a line of 20, its name's field, Test
    // and Hide, rows of 28 set in by 28; it grows away from the panel.
    let facts = session.wait_for(|facts| {
        mixer(facts).is_some_and(|(_, grown)| grown.y0 <= rect.y0 - 20.0 - 3.0 * 28.0)
    });
    let (_, rect) = mixer(&facts).expect("waited for");
    let set_in = rect.x0 + 13.0 + 28.0 + 20.0;
    let field = rect.y0 + two + 28.0 + 20.0 + 14.0;
    click(&session, surface, (set_in, field), PointerButton::Left);
    Host::new(&session.facts, &session.requests)
        .act(
            surface,
            &Verb::Type {
                text: "Desk\n".to_owned(),
            },
        )
        .expect("typed");
    let kept = || std::fs::read_to_string(&shell.state).unwrap_or_default();
    sound.until(|_| kept().contains("\"two\": \"Desk\""));
    let mode = std::fs::metadata(&shell.state)
        .expect("kept")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "for the person alone");
    click(
        &session,
        surface,
        (set_in, field + 2.0 * 28.0),
        PointerButton::Left,
    );
    sound.until(|_| kept().contains("\"hidden\": [\n    \"two\""));
    // Under "Hidden", folded, it and all it unfolded are gone from sight.
    let tall = rect.y1 - rect.y0;
    session.wait_for(|facts| {
        mixer(facts)
            .is_some_and(|(_, now)| now.y1 - now.y0 <= tall - 28.0 - 20.0 - 3.0 * 28.0 + 24.0)
    });

    // The person's press on the output, the mixer open, closes it and
    // switches: to nothing but "one", with "two" hidden. To an agent the
    // panel is under the mixer's surface while it is open.
    session.command(Command::Click {
        at: (1280 - 30 - 5, 800 - 20),
        button: perspicax_policy::Button::Left,
        mods: perspicax_policy::Mods::default(),
    });
    session.wait_for(|facts| mixer(facts).is_none());
    thread::sleep(Duration::from_millis(500));
    assert_eq!(sound.pactl(&["get-default-sink"]).trim(), "one");

    drop(sound);
    std::fs::remove_file(&shell.state).ok();
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

/// A panel of the mixer's microphone alone, at the panel's right end.
#[cfg(feature = "capture")]
const MIC_ONLY: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n\
    [shell.mixer]\npanel = [\"mic\"]\n";

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_click_on_the_microphone_mutes_the_default_input_and_the_wheel_moves_it() {
    use std::time::{Duration, Instant};

    use perspicax_policy::Mods;

    let sound = SoundServer::start("mic");
    sound.pactl(&[
        "load-module",
        "module-null-sink",
        "sink_name=mic",
        "media.class=Audio/Source/Virtual",
    ]);
    sound.until(|sound| sound.pactl(&["get-default-source"]).trim() == "mic");
    sound.pactl(&["set-source-volume", "mic", "40%"]);
    let session = Session::start("shell-mic", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "mic",
        MIC_ONLY,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    // The capsule, in the ink once the shell has heard the server.
    until_colour(&session, (1265, 800 - 25), INK);

    let until = |args: &[&str], want: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !sound.pactl(args).contains(want) {
            assert!(Instant::now() < deadline, "{args:?} never said {want}");
            thread::sleep(Duration::from_millis(50));
        }
    };
    click(
        &session,
        panel,
        (1265.0, PANEL_HEIGHT / 2.0),
        PointerButton::Left,
    );
    until(&["get-source-mute", "mic"], "Mute: yes");
    click(
        &session,
        panel,
        (1265.0, PANEL_HEIGHT / 2.0),
        PointerButton::Middle,
    );
    until(&["get-source-mute", "mic"], "Mute: no");
    session.command(Command::Scroll {
        at: (1265, 800 - 20),
        v120: (0, -120),
        mods: Mods::default(),
    });
    until(&["get-source-volume", "mic"], "/  45% /");
    assert!(
        sound.pactl(&["get-sink-mute", "one"]).contains("Mute: no"),
        "the output is left alone"
    );

    drop(sound);
    shell.stop_with(session);
}

/// A panel of the mixer's button and its speaker, the speaker at the
/// panel's right end and the button left of it.
#[cfg(feature = "capture")]
const OPEN_AND_MUTE: &str = "profile = \"classic\"\n[shell.panel]\nitems = [\"mixer\"]\n\
    [shell.mixer]\npanel = [\"open\", \"mute\"]\n";

/// The mixer, if it is open: its surface and its box.
#[cfg(feature = "capture")]
fn mixer(facts: &HostFacts) -> Option<(SurfaceId, Rect)> {
    facts
        .surfaces()
        .iter()
        .find_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Overlay,
                namespace,
            } if surface.mapped && namespace.starts_with("perspicax-mixer-") => {
                Some((surface.id, *surface.opaque.first()?))
            }
            _ => None,
        })
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn the_mixer_opens_from_its_button_mutes_and_sets_the_volume_and_closes() {
    use std::time::{Duration, Instant};

    let sound = SoundServer::start("mixer");
    sound.pactl(&["set-sink-volume", "one", "30%"]);
    let session = Session::start("shell-mixer", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "mixer",
        OPEN_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    until_colour(&session, (1258, 800 - 20), INK);
    let until = |args: &[&str], want: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !sound.pactl(args).contains(want) {
            assert!(Instant::now() < deadline, "{args:?} never said {want}");
            thread::sleep(Duration::from_millis(50));
        }
    };

    // Its button opens it, standing on the panel at the item's right end.
    let button = (1280.0 - 30.0 - 15.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, button, PointerButton::Left);
    let facts = session.wait_for(|facts| mixer(facts).is_some());
    let (surface, rect) = mixer(&facts).expect("waited for");
    assert_eq!((rect.x1, rect.y1), (1280.0, 800.0 - PANEL_HEIGHT));
    // The output's row: its mute button, then its slider. The layout's
    // numbers: 1 of edge and 12 of room, a 22-pixel title, a 32-pixel row,
    // a 28-pixel button and 8 between; the slider's track 7 in from each
    // of its ends, and 48 for the percentage, 8 from it.
    let row = rect.y0 + 13.0 + 22.0 + 16.0;
    click(
        &session,
        surface,
        (rect.x0 + 13.0 + 14.0, row),
        PointerButton::Left,
    );
    until(&["get-sink-mute", "one"], "Mute: yes");
    let track = (rect.x0 + 49.0 + 7.0, rect.x1 - 13.0 - 48.0 - 8.0 - 7.0);
    let half = (track.0 + track.1) / 2.0;
    click(&session, surface, (half, row), PointerButton::Left);
    until(&["get-sink-volume", "one"], "/  50% /");

    // Its button closes it again, for good, and a press off it does too.
    // The person's press: to an agent the panel is under the mixer's
    // surface while it is open.
    session.command(Command::Click {
        at: (1280 - 30 - 15, 800 - 20),
        button: perspicax_policy::Button::Left,
        mods: perspicax_policy::Mods::default(),
    });
    session.wait_for(|facts| mixer(facts).is_none());
    thread::sleep(Duration::from_millis(300));
    assert!(
        mixer(&session.wait_for(|_| true)).is_none(),
        "not opened again"
    );
    click(
        &session,
        panel,
        (1280.0 - 15.0, PANEL_HEIGHT / 2.0),
        PointerButton::Right,
    );
    let facts = session.wait_for(|facts| mixer(facts).is_some());
    let (surface, _) = mixer(&facts).expect("open");
    click(&session, surface, (100.0, 100.0), PointerButton::Left);
    session.wait_for(|facts| mixer(facts).is_none());

    // Open, the panel is still the panel's to a person's hand, which goes
    // by what is under it: a press on its speaker closes the mixer and
    // brings the sound back.
    click(&session, panel, button, PointerButton::Left);
    session.wait_for(|facts| mixer(facts).is_some());
    session.command(Command::Click {
        at: (1280 - 15, 800 - 20),
        button: perspicax_policy::Button::Left,
        mods: perspicax_policy::Mods::default(),
    });
    session.wait_for(|facts| mixer(facts).is_none());
    until(&["get-sink-mute", "one"], "Mute: no");

    drop(sound);
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_device_pressed_in_the_mixer_becomes_the_default_on_its_side() {
    let sound = SoundServer::start("mixer-list");
    for source in ["first", "second"] {
        sound.pactl(&[
            "load-module",
            "module-null-sink",
            &format!("sink_name={source}"),
            "media.class=Audio/Source/Virtual",
        ]);
    }
    sound.pactl(&["set-default-source", "first"]);
    sound.until(|sound| sound.pactl(&["get-default-source"]).trim() == "first");
    let session = Session::start("shell-mixer-list", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "mixer-list",
        OPEN_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    until_colour(&session, (1258, 800 - 20), INK);
    click(
        &session,
        panel,
        (1280.0 - 30.0 - 15.0, PANEL_HEIGHT / 2.0),
        PointerButton::Left,
    );
    let facts = session.wait_for(|facts| mixer(facts).is_some());
    let (surface, rect) = mixer(&facts).expect("waited for");
    // Under each side's title and its row of controls, 22 and 32 pixels, a
    // row of 28 for each device, in the order the server lists them; 8
    // between the sides, and 1 of edge and 12 of room round them all.
    let x = rect.x0 + 13.0 + 100.0;
    let one = rect.y0 + 13.0 + 22.0 + 32.0 + 14.0;
    let two = one + 28.0;
    // A device's round mark, filled in the accent for the one in use.
    let mark = |y: f64| ((rect.x0 + 13.0 + 14.0) as usize, y as usize);
    until_colour(&session, mark(one), ACCENT);
    click(&session, surface, (x, two), PointerButton::Left);
    sound.until(|sound| sound.pactl(&["get-default-sink"]).trim() == "two");
    until_colour(&session, mark(two), ACCENT);
    let second = rect.y0 + 13.0 + 2.0 * (22.0 + 32.0) + 3.0 * 28.0 + 8.0 + 14.0;
    click(&session, surface, (x, second), PointerButton::Left);
    sound.until(|sound| sound.pactl(&["get-default-source"]).trim() == "second");

    drop(sound);
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_program_playing_is_set_and_moved_to_another_output_from_the_mixer() {
    let mut sound = SoundServer::start("mixer-program");
    let silence = std::fs::File::open("/dev/zero").expect("/dev/zero");
    let player = sound
        .command("paplay")
        .args(["--raw", "--client-name=Tune"])
        .stdin(silence)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("paplay, which a desktop with sound has");
    sound.daemons.push(player);
    sound.until(|sound| sound.pactl(&["list", "sink-inputs"]).contains("Tune"));
    let session = Session::start("shell-mixer-program", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "mixer-program",
        OPEN_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    until_colour(&session, (1258, 800 - 20), INK);
    click(
        &session,
        panel,
        (1280.0 - 30.0 - 15.0, PANEL_HEIGHT / 2.0),
        PointerButton::Left,
    );
    // Its row is under the outputs' (a title, the controls, "one" and
    // "two"), the input's (a title and the controls), and the programs'
    // title, 8 between each section: 1 of edge and 12 of room, then 22,
    // 32 and 28 twice, 8, 22 and 32, 8 and 22.
    let row = 13.0 + 22.0 + 32.0 + 2.0 * 28.0 + 8.0 + 22.0 + 32.0 + 8.0 + 22.0;
    let facts = session
        .wait_for(|facts| mixer(facts).is_some_and(|(_, rect)| rect.y1 - rect.y0 > row + 60.0));
    let (surface, rect) = mixer(&facts).expect("waited for");
    let track = (rect.x0 + 49.0 + 7.0, rect.x1 - 13.0 - 48.0 - 8.0 - 7.0);
    let slider = rect.y0 + row + 28.0 + 16.0;
    click(
        &session,
        surface,
        ((track.0 + track.1) / 2.0, slider),
        PointerButton::Left,
    );
    sound.until(|sound| sound.pactl(&["list", "sink-inputs"]).contains("/  50% /"));
    click(
        &session,
        surface,
        (rect.x0 + 100.0, rect.y0 + row + 14.0),
        PointerButton::Left,
    );
    // Unfolded, it grows away from the panel by an output apiece.
    let facts = session
        .wait_for(|facts| mixer(facts).is_some_and(|(_, grown)| grown.y0 <= rect.y0 - 2.0 * 28.0));
    let (_, rect) = mixer(&facts).expect("waited for");
    let two = rect.y0 + row + 28.0 + 32.0 + 28.0 + 14.0;
    click(
        &session,
        surface,
        (rect.x0 + 100.0, two),
        PointerButton::Left,
    );
    let index = sound
        .pactl(&["list", "short", "sinks"])
        .lines()
        .find_map(|line| {
            let mut fields = line.split('\t');
            let index = fields.next()?;
            (fields.next()? == "two").then(|| index.to_owned())
        })
        .expect("two among the sinks");
    sound.until(|sound| {
        sound
            .pactl(&["list", "short", "sink-inputs"])
            .lines()
            .any(|line| line.split('\t').nth(1) == Some(index.as_str()))
    });

    drop(sound);
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn the_mixer_meters_what_plays_tests_an_output_and_stops_metering_once_closed() {
    let mut sound = SoundServer::start("meters");
    let silence = std::fs::File::open("/dev/zero").expect("/dev/zero");
    let player = sound
        .command("paplay")
        .args(["--raw", "--device=one"])
        .stdin(silence)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("paplay, which a desktop with sound has");
    sound.daemons.push(player);
    let session = Session::start("shell-meters", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "meters",
        OPEN_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    until_colour(&session, (1258, 800 - 20), INK);
    let button = (1280.0 - 30.0 - 15.0, PANEL_HEIGHT / 2.0);
    click(&session, panel, button, PointerButton::Left);
    let facts = session.wait_for(|facts| mixer(facts).is_some());
    let (surface, rect) = mixer(&facts).expect("waited for");
    let meters = |sound: &SoundServer| {
        sound
            .pactl(&["list", "source-outputs"])
            .matches("media.name = \"Meter\"")
            .count()
    };
    // "one", which plays, and not "two", which does not.
    sound.until(|sound| meters(sound) == 1);

    // "two"'s details: its key, a line of 20, its name's field, then Test.
    let two = 13.0 + 22.0 + 32.0 + 28.0;
    click(
        &session,
        surface,
        (rect.x1 - 13.0 - 14.0, rect.y0 + two + 14.0),
        PointerButton::Left,
    );
    let facts = session.wait_for(|facts| {
        mixer(facts).is_some_and(|(_, grown)| grown.y0 <= rect.y0 - 20.0 - 3.0 * 28.0)
    });
    let (_, rect) = mixer(&facts).expect("waited for");
    let test = rect.y0 + two + 28.0 + 20.0 + 28.0 + 14.0;
    click(
        &session,
        surface,
        (rect.x0 + 13.0 + 28.0 + 20.0, test),
        PointerButton::Left,
    );
    let index = sound
        .pactl(&["list", "short", "sinks"])
        .lines()
        .find_map(|line| {
            let mut fields = line.split('\t');
            let index = fields.next()?;
            (fields.next()? == "two").then(|| index.to_owned())
        })
        .expect("two among the sinks");
    // It sounds for a moment, on "two" and not the default: the one
    // stream of one channel of floats at 44.1 kHz.
    sound.until(|sound| {
        sound
            .pactl(&["list", "short", "sink-inputs"])
            .lines()
            .any(|line| {
                line.split('\t').nth(1) == Some(index.as_str())
                    && line.ends_with("float32le 1ch 44100Hz")
            })
    });

    // A press off it closes it.
    click(&session, surface, (100.0, 100.0), PointerButton::Left);
    session.wait_for(|facts| mixer(facts).is_none());
    sound.until(|sound| meters(sound) == 0);

    drop(sound);
    shell.stop_with(session);
}

/// What is shown on the screen a moment, the volume or a notice, if
/// anything is: its surface and where it is.
fn osd(facts: &HostFacts) -> Option<(SurfaceId, Rect)> {
    facts
        .surfaces()
        .iter()
        .find_map(|surface| match &surface.kind {
            SurfaceKind::Layer {
                layer: Layer::Overlay,
                namespace,
            } if surface.mapped && namespace == "perspicax-osd" => {
                Some((surface.id, surface.geometry))
            }
            _ => None,
        })
}

/// A keymap xkb cannot compile, as a save of the config asks for one.
fn refused_keymap(layout: &str) -> Command {
    Command::Keymap(Keymap {
        layout: layout.to_owned(),
        ..Keymap::default()
    })
}

/// Whether `rect` stands in the middle of a monitor 1280 wide, its foot
/// `above` the bottom of one 800 high.
fn in_the_middle(rect: Rect, above: f64) -> bool {
    ((rect.x0 + rect.x1) / 2.0 - 640.0).abs() <= 0.5 && rect.y1 == 800.0 - above
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn why_a_keymap_was_refused_is_shown_a_while_above_the_foot_of_the_screen() {
    use std::time::{Duration, Instant};

    let session = Session::start("shell-refused", Backend::headless((1280, 800)));
    // A shell of menus alone, with no panel and no sound, says it too.
    let shell = Shell::start(&session, "refused", "profile = \"minimal\"\n");
    session.wait_for(|facts| desktops(facts).len() == 1);

    session.command(refused_keymap("us,nowhere"));
    let facts = session.wait_for(|facts| osd(facts).is_some());
    let (_, rect) = osd(&facts).expect("waited for");
    assert!(in_the_middle(rect, 48.0), "{rect:?}");
    assert!(
        rect.x1 - rect.x0 > 280.0,
        "wider than the volume, for its words: {rect:?}"
    );
    assert_eq!(rect.y1 - rect.y0, 72.0, "two lines high: {rect:?}");

    let shown = Instant::now();
    let deadline = shown + Duration::from_secs(8);
    while osd(&session.wait_for(|_| true)).is_some() {
        assert!(Instant::now() < deadline, "never gone");
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        shown.elapsed() > Duration::from_secs(3),
        "there long enough to read: {:?}",
        shown.elapsed()
    );

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_refusal_while_one_is_shown_takes_its_place_at_its_own_width() {
    let session = Session::start("shell-refused-again", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "refused-again", CLASSIC);
    session.wait_for(|facts| panels(facts).len() == 1);

    session.command(refused_keymap("us,nowhere"));
    let facts = session.wait_for(|facts| osd(facts).is_some());
    let (first, rect) = osd(&facts).expect("waited for");
    assert!(
        in_the_middle(rect, PANEL_HEIGHT + 48.0),
        "above the panel: {rect:?}"
    );
    let wide = rect.x1 - rect.x0;

    // A longer layout's words are wider: the same surface, sized again.
    session.command(refused_keymap("us,nowhere,nor-anywhere-else-at-all"));
    let facts = session.wait_for(|facts| osd(facts).is_some_and(|(_, now)| now.x1 - now.x0 > wide));
    let (again, rect) = osd(&facts).expect("waited for");
    assert_eq!(again, first, "in its place");
    assert!(in_the_middle(rect, PANEL_HEIGHT + 48.0), "still: {rect:?}");

    shell.stop_with(session);
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_save_the_shell_cannot_use_is_said_on_the_screen_and_the_desktop_kept() {
    let session = Session::start("shell-refused-save", Backend::headless((1280, 800)));
    let shell = Shell::start(&session, "refused-save", "profile = \"minimal\"\n");
    let facts = session.wait_for(|facts| desktops(facts).len() == 1);
    let desktop = desktops(&facts);

    // A step the mixer refuses: the whole save is refused, and why is said
    // in a line, wider than the volume's box.
    shell.rewrite("profile = \"minimal\"\n[shell.mixer]\nstep = 99\n");
    session.command(Command::ReconfigureShell);
    let facts = session.wait_for(|facts| osd(facts).is_some());
    let (_, rect) = osd(&facts).expect("waited for");
    assert!(in_the_middle(rect, 48.0), "{rect:?}");
    assert!(rect.x1 - rect.x0 > 280.0, "as wide as its words: {rect:?}");
    assert_eq!(desktops(&facts), desktop, "and the desktop kept");

    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_change_from_elsewhere_shows_the_volume_a_moment_and_one_on_the_panel_does_not() {
    use std::time::{Duration, Instant};

    let sound = SoundServer::start("osd");
    let session = Session::start("shell-osd", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "osd",
        OPEN_AND_MUTE,
        Some(sound.socket()),
    );
    let facts = session.wait_for(|facts| panels(facts).len() == 1);
    let (_, panel, _) = panels(&facts)[0].clone();
    until_colour(&session, (1258, 800 - 20), INK);

    // Set elsewhere: shown in the middle, 48 above the panel, a moment.
    sound.pactl(&["set-sink-volume", "one", "40%"]);
    let facts = session.wait_for(|facts| osd(facts).is_some());
    let (_, rect) = osd(&facts).expect("waited for");
    assert_eq!(
        (rect.x0, rect.x1, rect.y1),
        (500.0, 780.0, 800.0 - PANEL_HEIGHT - 48.0),
        "{rect:?}"
    );
    let shown = Instant::now();
    session.wait_for(|facts| osd(facts).is_none());
    assert!(shown.elapsed() < Duration::from_secs(3), "gone again");

    // Muted on the panel, which shows it itself: nothing more.
    click(
        &session,
        panel,
        (1280.0 - 15.0, PANEL_HEIGHT / 2.0),
        PointerButton::Left,
    );
    sound.until(|sound| sound.pactl(&["get-sink-mute", "one"]).contains("yes"));
    thread::sleep(Duration::from_millis(500));
    assert!(
        osd(&session.wait_for(|_| true)).is_none(),
        "not for the panel's own"
    );

    drop(sound);
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn a_volume_key_steps_the_volume_and_shows_it() {
    let sound = SoundServer::start("keys");
    sound.pactl(&["set-sink-volume", "one", "40%"]);
    let session = Session::start("shell-keys", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "keys",
        OPEN_AND_MUTE,
        Some(sound.socket()),
    );
    session.wait_for(|facts| panels(facts).len() == 1);
    until_colour(&session, (1258, 800 - 20), INK);
    session.perform(Action::Volume(perspicax_policy::VolumeKey::Up));
    sound.until(|sound| {
        sound
            .pactl(&["get-sink-volume", "one"])
            .contains("/  45% /")
    });
    session.wait_for(|facts| osd(facts).is_some());

    drop(sound);
    shell.stop_with(session);
}

#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR, and PipeWire"]
fn with_no_mixer_on_the_panel_the_volume_is_still_shown_and_clicks_go_through_it() {
    use std::time::{Duration, Instant};

    let sound = SoundServer::start("osd-alone");
    let session = Session::start("shell-osd-alone", Backend::headless((1280, 800)));
    let shell = Shell::start_with(
        connect(session.socket()),
        "osd-alone",
        "profile = \"classic\"\n[shell.panel]\nitems = [\"clock\"]\n",
        Some(sound.socket()),
    );
    session.wait_for(|facts| panels(facts).len() == 1);
    // Nothing shows when the shell has first heard the server, which a
    // panel of no mixer does not show: set it until it is a change.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut loud = 30;
    let (_, rect) = loop {
        sound.pactl(&["set-sink-volume", "one", &format!("{loud}%")]);
        thread::sleep(Duration::from_millis(300));
        if let Some(shown) = osd(&session.wait_for(|_| true)) {
            break shown;
        }
        assert!(Instant::now() < deadline, "never shown");
        loud = 70 - loud;
    };
    // A person's right click on it reaches the desktop under it, which
    // opens its menu.
    let middle = ((rect.x0 + rect.x1) / 2.0, (rect.y0 + rect.y1) / 2.0);
    session.command(Command::Click {
        at: (middle.0 as i32, middle.1 as i32),
        button: perspicax_policy::Button::Right,
        mods: perspicax_policy::Mods::default(),
    });
    session.wait_for(|facts| menu(facts).is_some());

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
