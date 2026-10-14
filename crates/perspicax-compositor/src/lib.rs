//! `impl HostView` on Smithay -- the reference host.
//!
//! This crate answers the three questions the rest of the system cannot:
//! who owns a surface, whether a rect on it can actually be seen, and how to
//! put input into the seat so that focus, grabs and z-order stay correct by
//! construction rather than by imitation.
//!
//! The signal worth building for is the disagreement between two feeds: surface
//! damage says *something changed on screen*, accessibility events say *what
//! changed semantically*. Damage arriving with no accompanying event is the
//! precise definition of a surface that renders without explaining itself, and
//! the only honest trigger for a vision fallback.
//!
//! # Headless, it does not draw anything, and that is the design
//!
//! A compositor is usually a thing that composites. This one runs a Wayland
//! server, tracks surfaces, geometry, regions, z-order and damage, and never
//! puts a pixel anywhere. Everything M2 has to answer -- who drew this, can it
//! be seen, has it changed -- is bookkeeping the server does on the way past;
//! none of it needs the picture.
//!
//! What that buys is not elegance, it is dependencies. No renderer means no
//! EGL, no GL, no GBM, no DRM and no GPU, so the whole thing runs in a
//! container as ordinary software, which is what makes the M2 demo a CI job
//! rather than a machine somebody has to keep. The one system library left is
//! `libxkbcommon`, which Smithay links unconditionally for keymaps.
//!
//! It also fixes a real behaviour: because nothing is ever read from a client's
//! buffer, buffers are released the moment they arrive rather than one commit
//! late. See [`state::Compositor::commit`].
//!
//! A person needs the picture, so [`Backend::Seat`] -- behind the `seat`
//! feature -- renders to the outputs a GPU has connected and takes input from
//! libinput. It is a second backend beside this one, not a replacement for
//! it: the headless compositor stays renderer-free, and it stays what CI runs.

mod access;
pub mod act;
mod backend;
mod capture;
mod damage;
mod decorations;
pub mod facts;
mod focus;
mod framed;
mod geometry;
mod grants;
mod heads;
mod hold;
pub mod host;
mod keyboard;
mod layers;
mod lock;
mod mouse;
mod origin;
mod output_management;
mod outputs;
mod pager;
mod popup;
mod scale;
#[cfg(feature = "capture")]
mod screencopy;
mod shell;
mod shell_protocol;
mod sizes;
mod stall;
pub mod state;
mod toplevels;
#[cfg(feature = "xwayland")]
mod xwayland;

pub use crate::{
    act::{ActError, Dispatched},
    backend::{Agent, Backend, Virtual},
    facts::{Facts, SessionFacts},
    host::{Command, Host, LaunchError, Request, Requests},
    keyboard::Keymap,
};

use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _, process::CommandExt as _},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::{
    reexports::{
        calloop::{
            EventLoop, Interest, Mode as PollMode, PostAction, channel::Event as ChannelEvent,
            generic::Generic,
        },
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::{backend::Running, host::Inbound, state::Compositor};

/// How often a headless compositor tells clients they may draw again, and the
/// longest the loop sleeps between checks for a stop. 60 Hz, because that is
/// what a toolkit expects and a slower tick would make every damage
/// measurement in this milestone a measurement of this constant instead.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How much may wait for a client that has stopped reading before it is cut
/// off. wayland-backend's own is 4 KB, so a client was cut off once the
/// kernel's buffer to it filled too: a few seconds of a pointer moving over
/// it. That dropped #80's frozen Flutter applications, and the shell when
/// something kept it from reading (#101). A megabyte is about half a minute
/// of that, and costs only a client that falls behind: the buffer grows as
/// it is needed.
const MOST_UNREAD: usize = 1 << 20;

/// What shows where no window is: a dark grey, so a working output is
/// distinguishable from a dead one, on a monitor and in a picture of one. The
/// wallpaper is the shell's job (W5).
#[cfg_attr(
    not(any(feature = "seat", feature = "capture")),
    expect(dead_code, reason = "drawn only by a seat or a picture")
)]
pub(crate) const BACKDROP: [f32; 4] = [0.12, 0.12, 0.14, 1.0];

/// A request for a running compositor to stop.
///
/// Separate from [`Config::run_for`] because the two answer different
/// questions. A deadline is a guess made before anything started; this is a
/// decision made by whoever is actually doing the work -- and the work that
/// matters here, reading two accessibility trees, takes seconds it cannot
/// predict. A compositor that exited on a guessed deadline mid-read would take
/// its clients down with it and the read would fail for a reason nothing
/// upstream could see.
#[derive(Debug, Clone, Default)]
pub struct Stop(Arc<AtomicBool>);

impl Stop {
    /// A request nobody has made yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the compositor to stop after its current turn round the loop.
    pub fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether anyone has asked.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// What went wrong bringing a compositor up.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The event loop could not be created, or a source could not be added.
    #[error("the compositor's event loop failed: {0}")]
    EventLoop(String),
    /// The Wayland display itself could not be created.
    #[error("could not create the Wayland display: {0}")]
    Display(String),
    /// No Wayland socket could be bound. Nearly always a missing or unwritable
    /// `XDG_RUNTIME_DIR`.
    #[error("no Wayland socket could be bound: {0}")]
    Socket(String),
    /// A child named by `--spawn` could not be started.
    #[error("could not spawn `{command}`: {source}")]
    Spawn {
        /// The command, as given.
        command: String,
        /// Why it did not start.
        source: std::io::Error,
    },
    /// The backend asked for was left out of this build.
    #[error(
        "this perspicax was built without the {backend} backend; rebuild with \
         the `{feature}` cargo feature (from the workspace root, \
         `--features perspicax/desktop` builds a full session)"
    )]
    NotBuilt {
        /// The backend, as the CLI spells it.
        backend: &'static str,
        /// The cargo feature that provides it.
        feature: &'static str,
    },
    /// The seat could not be brought up: no session, no GPU, no input.
    #[error("the seat could not be brought up: {0}")]
    Seat(String),
    /// The config file could not be used. Refused at start, rather than
    /// starting a session that silently ignores what the person wrote.
    #[error("config: {0}")]
    Config(String),
    /// The display failed while talking to clients.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How to run. The default is a 1920x1080 headless compositor that spawns
/// nothing and runs until stopped.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Where output goes and input comes from. See [`Backend`].
    pub backend: Backend,
    /// Commands to start once the socket exists -- and Xwayland, when this
    /// compositor starts one -- each as a program and its arguments.
    pub spawn: Vec<Vec<String>>,
    /// Environment every spawned child gets on top of this process's own.
    ///
    /// Here rather than inherited, because the compositor is the session
    /// manager for what it spawns, and the variables that decide whether a
    /// toolkit joins the accessibility bus are exactly the kind that a desktop
    /// sets invisibly and a container does not set at all.
    pub env: Vec<(String, String)>,
    /// Stop after this long. `None` runs until killed, which is what a session
    /// wants; a test wants a bound.
    pub run_for: Option<Duration>,
    /// The seat's `config.toml`, read at start and again on the reload
    /// binding. A file that does not exist is the classic profile. Headless
    /// reads no config at all: what CI and an agent run has to be the same
    /// on every machine, whatever its owner likes their focus model to be.
    pub config: Option<PathBuf>,
    /// The Wayland socket's name in `XDG_RUNTIME_DIR`, instead of the first
    /// free `wayland-N`. For whoever has to connect without being spawned
    /// by us: a test's own client, or a second session beside a first.
    pub socket: Option<String>,
    /// Start Xwayland headless too, for X11 applications an agent wants to
    /// host. Off by default, so what CI runs needs no X server. A seat
    /// decides from its config (`xwayland`), not from this. Ignored in a
    /// build without the `xwayland` feature.
    pub xwayland: bool,
}

/// Run a compositor until its deadline passes, or forever.
///
/// Three handles, all owned by the caller, and each one a direction: `facts` is
/// what this loop publishes outward, `requests` is what it accepts inward, and
/// `stop` is how it is asked to finish. The caller holds all three so that
/// whoever reads the facts, drives the acting, and decides when there is
/// nothing left to do need not be the thread running this loop -- and cannot
/// be, since none of the state below is `Send`.
///
/// # Errors
///
/// [`Error`], for any of the ways a compositor fails to come up: a backend
/// this build left out, no socket, no event loop, a child that will not start,
/// or a display that fails mid-run.
pub fn run(config: &Config, facts: &Facts, requests: &Requests, stop: &Stop) -> Result<(), Error> {
    let mut event_loop: EventLoop<'static, Compositor> =
        EventLoop::try_new().map_err(|error| Error::EventLoop(error.to_string()))?;
    let mut display: Display<Compositor> =
        Display::new().map_err(|error| Error::Display(error.to_string()))?;
    let handle = display.handle();
    // Before any client is accepted, Xwayland included: each takes it as it
    // connects.
    handle.set_default_max_buffer_size(MOST_UNREAD);
    // Before the socket, so a seat that cannot come up -- no session, no GPU,
    // no monitor -- is refused before any client has been accepted by a
    // compositor that cannot show it anything.
    let backend = Running::start(config, &handle, &event_loop.handle())?;
    let mut state = Compositor::new(&handle, event_loop.handle(), backend, facts.clone());
    Running::attach(&mut state, &event_loop.handle())?;
    // Once before any client, so a reader sees this host's consent policy and
    // outputs from the start rather than an empty default until something
    // happens to change.
    state.publish_facts();

    let socket = match &config.socket {
        Some(name) => ListeningSocketSource::with_name(name),
        None => ListeningSocketSource::new_auto(),
    }
    .map_err(|error| Error::Socket(error.to_string()))?;
    let socket_name = socket.socket_name().to_os_string();
    event_loop
        .handle()
        .insert_source(socket, |stream, (), state: &mut Compositor| {
            state.insert_client(stream);
        })
        .map_err(|error| Error::EventLoop(error.to_string()))?;

    // The display's own file descriptor, registered only so that a client
    // becoming readable wakes the loop; the dispatch itself happens below,
    // outside any callback.
    //
    // The usual way to write this hands the `Display` to calloop as source data
    // and reaches it again through `Generic::get_mut`, which is `unsafe`. This
    // crate is the one in the workspace allowed to write `unsafe`, and it
    // spends that only where a C API leaves no choice (EGL, in the seat
    // backend) -- keeping the display in a local and only *polling* its
    // descriptor here is why this is not one of those places.
    let poll_fd = display.backend().poll_fd().try_clone_to_owned()?;
    event_loop
        .handle()
        .insert_source(
            Generic::new(poll_fd, Interest::READ, PollMode::Level),
            |_, _, _: &mut Compositor| Ok(PostAction::Continue),
        )
        .map_err(|error| Error::EventLoop(error.to_string()))?;

    // The inbound arrow. Everything else this loop listens to is something a
    // client did; this is the one source carrying a request from our own
    // process, and it is what makes the compositor drivable rather than only
    // readable. The work happens here, on this thread, because `Compositor` is
    // not `Send` and no lock could change that.
    //
    // `take_inbox` returning `None` means a second `run` against one `Requests`.
    // That is a caller error rather than a runtime condition, and the loop
    // still comes up: it simply accepts no actions, and every `Host::act`
    // against it times out saying so.
    if let Some(inbox) = requests.take_inbox() {
        event_loop
            .handle()
            .insert_source(inbox, |event, (), state: &mut Compositor| {
                let request = match event {
                    ChannelEvent::Msg(Inbound::Act(request)) => request,
                    ChannelEvent::Msg(Inbound::Command(command)) => {
                        state.command(&command);
                        return;
                    }
                    ChannelEvent::Msg(Inbound::Capture(request)) => {
                        let shot = state.capture(&request.target);
                        if let Err(ref error) = shot {
                            tracing::warn!(%error, "capture refused");
                        }
                        let _ = request.reply.send(shot);
                        return;
                    }
                    ChannelEvent::Msg(Inbound::Launch(request)) => {
                        let launched =
                            state.launch_for_agent(&request.command, request.cwd.as_deref());
                        if let Err(ref error) = launched {
                            tracing::warn!(%error, "launch refused");
                        }
                        let _ = request.reply.send(launched);
                        return;
                    }
                    ChannelEvent::Closed => return,
                };
                let outcome = state.act(request.surface, &request.action);
                if let Err(ref error) = outcome {
                    tracing::warn!(surface = request.surface.0, %error, "act refused");
                }
                // Acting can move focus, and focus is one of the facts the join
                // weighs. Publish before answering, so a caller that reads the
                // facts the moment its receipt arrives sees the world the
                // receipt describes rather than the one before it.
                state.publish_facts();
                // A caller that stopped waiting has dropped the receiver. That
                // is its right, and there is nobody left to tell.
                let _ = request.reply.send(outcome);
            })
            .map_err(|error| Error::EventLoop(error.to_string()))?;
    } else {
        tracing::warn!("this Requests already has a loop: no actions will be accepted");
    }

    tracing::info!(socket = ?socket_name, backend = ?config.backend, "compositor up");
    // Before anything is started, so whoever passes it on -- to the session
    // bus, for programs D-Bus starts -- has it before any program could ask.
    facts.publish_session(|session| {
        session.wayland_display = Some(socket_name.to_string_lossy().into_owned());
    });
    state.launch = Some(Launch {
        socket: socket_name.clone(),
        env: config.env.clone(),
        x11_display: None,
        look: Vec::new(),
    });
    // The agent's programs, then on a seat the person's own, which are never
    // granted consent. Where this compositor starts an Xwayland both wait for
    // it, so an X11 program -- or one started from a spawned terminal --
    // finds a `DISPLAY`. Its number is not known until it is ready.
    let spawn = config.spawn.clone();
    Running::populate(
        &mut state,
        &event_loop.handle(),
        config.xwayland,
        move |state| spawn_all(state, &spawn),
    )?;

    let deadline = config.run_for.map(|run_for| Instant::now() + run_for);
    let result = loop {
        if let Err(error) = event_loop.dispatch(Some(FRAME_INTERVAL), &mut state) {
            break Err(Error::EventLoop(error.to_string()));
        }
        if let Err(error) = display.dispatch_clients(&mut state) {
            break Err(Error::Io(error));
        }
        state.popups.cleanup();
        state.announce_layout();
        // Before the next dispatch can accept a client: see `reap_spawned`.
        state.reap_spawned();
        state.backend.reap();
        // Each client of ours on its own, to see which have stopped reading;
        // then all of them, Xwayland included, as before.
        state.flush_each();
        if let Err(error) = display.flush_clients() {
            break Err(Error::Io(error));
        }
        // Before the stops, so a deadline cannot pass off a session that
        // never started what it was asked to as one that ran.
        if let Some(error) = state.spawn_failed.take() {
            break Err(error);
        }
        if stop.requested()
            || state.exit_asked
            || state.backend.exit_requested()
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            break Ok(());
        }
    };

    // Children are killed rather than left behind. A compositor that exits
    // owing a live GTK window to a socket nobody is listening on has produced a
    // process that will never be told to stop.
    for child in &mut state.spawned {
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(feature = "xwayland")]
    xwayland::stop(&mut state, &event_loop.handle());
    result
}

/// Start every `--spawn` command against this compositor's socket, as a key
/// binding would: `DISPLAY` included, once there is an Xwayland to name.
///
/// What starts is what an agent may act on, on a seat, so consent is granted
/// here, the moment each pid exists, until the process exits. Headless, that
/// changes nothing: consent there is already everyone.
///
/// The first command that will not start stops the rest, and is left for
/// `run` to return. What did start is kept in `state.spawned`, so it is
/// reaped as it exits, and killed with the session rather than left behind.
///
/// The two toolkit variables are set because the default is not "whatever is
/// running": a GTK or Qt client with `DISPLAY` set and no instruction will pick
/// X11, connect to whatever X server is around, and map its window somewhere
/// this compositor cannot see -- which looks exactly like a client that failed
/// to start.
fn spawn_all(state: &mut Compositor, commands: &[Vec<String>]) {
    if state.launch.is_none() {
        return;
    }
    for command in commands.iter().filter(|command| !command.is_empty()) {
        if let Err(error) = state.start_for_agent(command, None, grants::Why::Spawn) {
            state.spawn_failed = Some(error);
            break;
        }
    }
}

/// What a program started by this compositor needs to find it: the socket,
/// and the environment every child gets. Kept on the compositor so a key
/// binding or an autostart entry starts programs exactly as `--spawn` does.
#[derive(Debug, Clone)]
pub(crate) struct Launch {
    socket: OsString,
    env: Vec<(String, String)>,
    /// Xwayland's display, once it is up. Without one, `DISPLAY` is removed,
    /// so an X11 program fails plainly rather than finding some other X
    /// server and drawing where this compositor cannot see.
    pub(crate) x11_display: Option<u32>,
    /// What the session's look says to every program it starts: the
    /// pointer's theme and size, from the theme, on a seat. Changed by a
    /// reload, and seen by what starts after it.
    pub(crate) look: Vec<(String, String)>,
}

impl Launch {
    /// Start one program, as a program and its arguments.
    #[cfg_attr(
        not(any(feature = "seat", test)),
        expect(dead_code, reason = "the seat's shell supervisor")
    )]
    pub(crate) fn spawn(&self, command: &[impl AsRef<OsStr>]) -> Result<Child, Error> {
        self.spawn_in(command, None)
    }

    /// Start one program in `dir`, or where this process is if `None`: a
    /// desktop entry's `Path`, or the folder an agent named.
    ///
    /// A bare name is looked for here, before the program starts, in the
    /// folders of the session's `PATH` that are whole paths, and the file
    /// found is started under that name, as a shell would start it. Left to
    /// the start, it would be looked for after the change to `dir`, where a
    /// folder of `PATH` such as `.` or `bin` names one inside `dir`, so
    /// whoever chose the folder would choose the program.
    ///
    /// It reads nothing, and what it prints goes where this process's own log
    /// goes, its stderr (issue #43). Under `--mcp` this process's stdin and
    /// stdout are an agent's JSON-RPC wire, and a program that inherited them
    /// could take the agent's requests, or print a line between the replies
    /// -- one that, being valid JSON-RPC, would pass for perspicax's own. In
    /// every mode rather than only that one, so what CI runs is what a seat
    /// runs, and a program started from a key binding is started the same way
    /// as one an agent asked for.
    pub(crate) fn spawn_in(
        &self,
        command: &[impl AsRef<OsStr>],
        dir: Option<&Path>,
    ) -> Result<Child, Error> {
        let shown = command
            .iter()
            .map(|word| word.as_ref().to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let (program, arguments) = command.split_first().ok_or_else(|| Error::Spawn {
            command: String::new(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "an empty command"),
        })?;
        let program = program.as_ref();
        let mut command_line = if program.as_bytes().contains(&b'/') {
            std::process::Command::new(program)
        } else {
            let found = self.find(program).ok_or_else(|| Error::Spawn {
                command: shown.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "in no folder of PATH named by a whole path",
                ),
            })?;
            let mut command_line = std::process::Command::new(found);
            command_line.arg0(program);
            command_line
        };
        command_line
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(io::stderr())
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .envs(self.look.iter().map(|(key, value)| (key, value)))
            .env("WAYLAND_DISPLAY", &self.socket)
            // Wayland first, and X11 for a toolkit that will not: Chromium and
            // Electron in X11 mode allow GTK only its X11 backend, and a strict
            // `wayland` leaves them nothing to open. The fallback can reach no
            // X server but this compositor's own, since `DISPLAY` is either its
            // Xwayland or removed.
            .env("GDK_BACKEND", "wayland,x11")
            .env("QT_QPA_PLATFORM", "wayland;xcb");
        if let Some(dir) = dir {
            command_line.current_dir(dir);
        }
        match self.x11_display {
            Some(display) => command_line.env("DISPLAY", format!(":{display}")),
            None => command_line.env_remove("DISPLAY"),
        };
        let child = command_line.spawn().map_err(|source| Error::Spawn {
            command: shown.clone(),
            source,
        })?;
        tracing::info!(pid = child.id(), command = %shown, "spawned");
        Ok(child)
    }

    /// An environment variable as the programs this starts are given it: the
    /// session's own value where it sets one, else this process's.
    pub(crate) fn var(&self, name: &str) -> Option<String> {
        self.env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var(name).ok())
    }

    /// Where `program`, a bare name, is: the first file of that name that
    /// may be run, in a folder of the session's `PATH` that is a whole path.
    fn find(&self, program: &OsStr) -> Option<PathBuf> {
        let path = self.var("PATH")?;
        std::env::split_paths(&path)
            .filter(|folder| folder.is_absolute())
            .map(|folder| folder.join(program))
            .find(|candidate| runnable(candidate))
    }
}

/// Whether `path` is a file that may be run.
pub(crate) fn runnable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs, io,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use super::{Error, Launch};

    /// Set for the copy of this binary that [`launch_probe`] runs in.
    const PROBE: &str = "PERSPICAX_LAUNCH_PROBE";

    /// Where `fd` of process `pid` points: `/dev/null`, or `pipe:[N]`.
    fn link(pid: &str, fd: u8) -> PathBuf {
        fs::read_link(format!("/proc/{pid}/fd/{fd}")).expect("a /proc fd link")
    }

    /// Run in a copy of this binary whose stdin, stdout and stderr are three
    /// different pipes. In this one they may not be: a runner whose stdin is
    /// already `/dev/null`, or whose stdout and stderr are one terminal, would
    /// pass with the program inheriting both, and prove nothing.
    #[test]
    fn a_program_started_reads_nothing_and_writes_where_the_log_goes() {
        let probe = Command::new(std::env::current_exe().expect("this test's binary"))
            .args(["--exact", "tests::launch_probe", "--ignored"])
            .args(["--test-threads=1", "--nocapture"])
            .env(PROBE, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("the probe runs");
        let said = String::from_utf8_lossy(&probe.stdout);
        // A filter that matched nothing passes too, having run nothing.
        assert!(
            probe.status.success() && said.contains("1 passed"),
            "{said}{}",
            String::from_utf8_lossy(&probe.stderr)
        );
    }

    #[test]
    #[ignore = "run by a_program_started_reads_nothing_and_writes_where_the_log_goes"]
    fn launch_probe() {
        if std::env::var_os(PROBE).is_none() {
            return;
        }
        assert_ne!(link("self", 0), Path::new("/dev/null"), "stdin is a pipe");
        assert_ne!(link("self", 1), link("self", 2), "stdout is not stderr");

        let launch = Launch {
            socket: "wayland-probe".into(),
            env: Vec::new(),
            x11_display: None,
            look: Vec::new(),
        };
        // `spawn` returns once the program is running, its fds settled.
        let mut child = launch.spawn(&["sleep", "30"]).expect("sleep starts");
        let pid = child.id().to_string();
        let (stdin, stdout) = (link(&pid, 0), link(&pid, 1));
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(stdin, Path::new("/dev/null"), "it has nothing to read");
        assert_eq!(stdout, link("self", 2), "it prints to this one's stderr");
    }

    /// A fresh folder for one test, removed when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                env::temp_dir().join(format!("perspicax-launch-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch folder");
            Self(dir)
        }

        /// A file at `path` in it, with `mode`: a program, if that says it
        /// may be run.
        fn file(&self, path: &str, mode: u32) -> PathBuf {
            let file = self.0.join(path);
            fs::create_dir_all(file.parent().expect("a folder")).expect("its folder");
            fs::write(&file, "#!/bin/sh\nexec sleep 30\n").expect("a file");
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).expect("its mode");
            file
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// What starts programs with `PATH` set to `path`.
    fn on_path(path: &str) -> Launch {
        Launch {
            socket: "wayland-probe".into(),
            env: vec![("PATH".to_owned(), path.to_owned())],
            x11_display: None,
            look: Vec::new(),
        }
    }

    #[test]
    fn a_bare_name_is_found_only_in_the_folders_of_path_that_are_whole_paths() {
        let scratch = Scratch::new("find");
        let program = scratch.file("bin/perspicax-found", 0o755);
        scratch.file("bin/perspicax-unrunnable", 0o644);
        let bin = scratch.0.join("bin");
        // The same folder, named from where this test runs.
        let here = env::current_dir().expect("a folder to run in");
        let relative = Path::new(&"../".repeat(here.components().count() - 1))
            .join(bin.strip_prefix("/").expect("a whole path"));
        assert!(
            relative.join("perspicax-found").is_file(),
            "it names the folder"
        );

        let found = |path: String, program: &str| on_path(&path).find(program.as_ref());
        let (bin, relative) = (bin.display(), relative.display());
        assert_eq!(found(format!("{relative}"), "perspicax-found"), None);
        assert_eq!(
            found(format!("{relative}:/nonexistent:{bin}"), "perspicax-found"),
            Some(program)
        );
        assert_eq!(found(format!("{bin}"), "perspicax-unrunnable"), None);
    }

    /// What the lookup is made before the start for: a folder of `PATH`
    /// such as `bin`, looked in once the program's own folder is changed to,
    /// would find a program there.
    #[test]
    fn a_program_in_the_folder_it_starts_in_is_not_found_by_a_folder_of_path_inside_it() {
        let scratch = Scratch::new("inside");
        scratch.file("bin/perspicax-only-here", 0o755);
        let started =
            on_path("bin:/usr/bin:/bin").spawn_in(&["perspicax-only-here"], Some(&scratch.0));
        match started {
            Err(Error::Spawn { command, source }) => {
                assert_eq!(command, "perspicax-only-here");
                assert_eq!(source.kind(), io::ErrorKind::NotFound, "{source}");
            }
            Ok(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("started from inside the folder it was started in");
            }
            Err(other) => panic!("{other}"),
        }
    }

    #[test]
    fn a_program_found_starts_in_its_folder_under_the_name_it_was_asked_by() {
        let scratch = Scratch::new("arg0");
        let launch = Launch {
            env: Vec::new(),
            ..on_path("")
        };
        let mut child = launch
            .spawn_in(&["sleep", "30"], Some(&scratch.0))
            .expect("sleep starts");
        let pid = child.id();
        let asked = b"sleep\x0030\x00";
        // Read once the new program is there. Spawning can return before the
        // exec is done: for a moment the child's command line is still this
        // process's, then it is empty until the new arguments are copied in.
        let deadline = Instant::now() + Duration::from_secs(5);
        let said = loop {
            let said = fs::read(format!("/proc/{pid}/cmdline")).expect("its command line");
            if said == asked || Instant::now() >= deadline {
                break said;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let folder = fs::read_link(format!("/proc/{pid}/cwd")).expect("its folder");
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(said, asked, "as a shell would name it");
        assert_eq!(folder, scratch.0);
    }
}
