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
//! # It does not draw anything, and that is the design
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
//! Real DRM, modesetting and multi-output wait for `--seat`; they are the part
//! of compositor work that consumes schedule without proving anything.

pub mod act;
pub mod facts;
pub mod host;
mod origin;
pub mod state;

pub use crate::{
    act::{ActError, Dispatched},
    facts::Facts,
    host::{Host, Request, Requests},
};

use std::{
    ffi::OsString,
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::{
    reexports::{
        calloop::{
            EventLoop, Interest, Mode as PollMode, PostAction,
            channel::Event as ChannelEvent,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Display,
    },
    wayland::socket::ListeningSocketSource,
};

use crate::state::Compositor;

/// How often clients are told they may draw again. 60 Hz, because that is what
/// a toolkit expects and a slower tick would make every damage measurement in
/// this milestone a measurement of this constant instead.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

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
    /// The display failed while talking to clients.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How to run.
#[derive(Debug, Clone)]
pub struct Config {
    /// The virtual output's size in pixels. Windows are placed inside it, and
    /// it is the coordinate space every global rect in [`perspicax_index::HostFacts`]
    /// is expressed in.
    pub size: (i32, i32),
    /// Commands to start once the socket exists, each as a program and its
    /// arguments.
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            size: (1920, 1080),
            spawn: Vec::new(),
            env: Vec::new(),
            run_for: None,
        }
    }
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
/// [`Error`], for any of the ways a compositor fails to come up: no socket, no
/// event loop, a child that will not start, or a display that fails mid-run.
pub fn run(config: &Config, facts: &Facts, requests: &Requests, stop: &Stop) -> Result<(), Error> {
    let mut event_loop: EventLoop<'static, Compositor> =
        EventLoop::try_new().map_err(|error| Error::EventLoop(error.to_string()))?;
    let mut display: Display<Compositor> =
        Display::new().map_err(|error| Error::Display(error.to_string()))?;
    let handle = display.handle();
    let mut state = Compositor::new(&handle, config.size, facts.clone());

    let socket =
        ListeningSocketSource::new_auto().map_err(|error| Error::Socket(error.to_string()))?;
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
    // crate is the one in the workspace allowed to write `unsafe`, and it has
    // not needed to yet -- keeping the display in a local and only *polling*
    // its descriptor here is why.
    let poll_fd = display.backend().poll_fd().try_clone_to_owned()?;
    event_loop
        .handle()
        .insert_source(
            Generic::new(poll_fd, Interest::READ, PollMode::Level),
            |_, _, _: &mut Compositor| Ok(PostAction::Continue),
        )
        .map_err(|error| Error::EventLoop(error.to_string()))?;

    event_loop
        .handle()
        .insert_source(Timer::immediate(), |_, (), state: &mut Compositor| {
            state.send_frames();
            TimeoutAction::ToDuration(FRAME_INTERVAL)
        })
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
                let ChannelEvent::Msg(request) = event else {
                    return;
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

    tracing::info!(socket = ?socket_name, size = ?config.size, "compositor up");
    let mut children = spawn_all(&config.spawn, &config.env, &socket_name)?;

    let deadline = config.run_for.map(|run_for| Instant::now() + run_for);
    let result = loop {
        if let Err(error) = event_loop.dispatch(Some(FRAME_INTERVAL), &mut state) {
            break Err(Error::EventLoop(error.to_string()));
        }
        if let Err(error) = display.dispatch_clients(&mut state) {
            break Err(Error::Io(error));
        }
        if let Err(error) = display.flush_clients() {
            break Err(Error::Io(error));
        }
        if stop.requested() || deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break Ok(());
        }
    };

    // Children are killed rather than left behind. A compositor that exits
    // owing a live GTK window to a socket nobody is listening on has produced a
    // process that will never be told to stop.
    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Start every `--spawn` command against this compositor's socket.
///
/// The two toolkit variables are set because the default is not "whatever is
/// running": a GTK or Qt client with `DISPLAY` set and no instruction will pick
/// X11, connect to whatever X server is around, and map its window somewhere
/// this compositor cannot see -- which looks exactly like a client that failed
/// to start.
fn spawn_all(
    commands: &[Vec<String>],
    env: &[(String, String)],
    socket: &OsString,
) -> Result<Vec<Child>, Error> {
    let mut children = Vec::with_capacity(commands.len());
    for command in commands {
        let Some((program, arguments)) = command.split_first() else {
            continue;
        };
        let child = Command::new(program)
            .args(arguments)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .env("WAYLAND_DISPLAY", socket)
            .env("GDK_BACKEND", "wayland")
            .env("QT_QPA_PLATFORM", "wayland")
            .env_remove("DISPLAY")
            .spawn()
            .map_err(|source| Error::Spawn {
                command: command.join(" "),
                source,
            })?;
        tracing::info!(pid = child.id(), command = %command.join(" "), "spawned");
        children.push(child);
    }
    Ok(children)
}
