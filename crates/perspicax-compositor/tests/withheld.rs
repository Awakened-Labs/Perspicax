//! Windows that open without the keys, by a compositor with no screen and a
//! person at its seat (#259).
//!
//! A window used to take the keys as it opened, whatever the person was
//! doing: a password typed at a terminal's `sudo` prompt went, from the
//! moment some other program mapped a window, to that program, Enter and
//! all. Now a window takes the keys as it opens only if the person asked for
//! it: a window of the program they are using, anything while nothing has
//! the keys, or one their latest press reached or started -- a panel
//! clicked, an Enter at a terminal, a binding. Any other opens just beneath
//! the window in use, not told it is active, and has the keys only once the
//! person gives them to it. A token is the other way of asking, spent once
//! it is open. With nobody at the seat, every window takes the keys, as
//! before.
//!
//! Each test has programs, each a client of its own with a window and a
//! keyboard, and some a program in a process of its own: a copy of this
//! binary running [`window_probe`], started by this one, as a terminal
//! starts what is typed at it. What each keyboard was told is what is
//! asserted, `enter` and `leave` alone; for a probe's window, which the
//! facts say it was last given the keys. Which window is on top, and which
//! is over which, are the facts', bottom to top. Only the person's hand, a
//! stand-in clicking, needs `capture`, to find what is under it; their keys
//! need nothing. Like the other live tests these bind a real Wayland socket,
//! so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use common::{Desk, Ear, Session, heard, until};
use perspicax_compositor::{Backend, Command as Do};
use perspicax_index::{HostFacts, SurfaceFacts};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{EventQueue, Proxy as _, QueueHandle, globals::GlobalList};

/// Set, to the compositor's socket, for the copy of this binary that
/// [`window_probe`] runs in.
const PROBE: &str = "PERSPICAX_WITHHELD_PROBE";
/// What the probe does, and in what order: see [`Steps`].
const STEPS: &str = "PERSPICAX_WITHHELD_STEPS";
/// A file the probe waits for before its next step, where it has one.
const GATE: &str = "PERSPICAX_WITHHELD_GATE";

/// A session with a person at the seat.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title) && surface.mapped)
}

/// The title of the window on top, the facts giving them bottom to top.
fn on_top(facts: &HostFacts) -> Option<String> {
    facts
        .surfaces()
        .iter()
        .rfind(|surface| surface.mapped && surface.title.is_some())?
        .title
        .clone()
}

/// The mapped windows' titles, bottom to top.
fn stack(facts: &HostFacts) -> Vec<String> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| surface.mapped)
        .filter_map(|surface| surface.title.clone())
        .collect()
}

/// Whether the facts say the window `title` was ever given the keys.
fn given_keys(facts: &HostFacts, title: &str) -> bool {
    titled(facts, title).is_some_and(|window| window.focused_at.is_some())
}

/// A program: a client of its own, with a keyboard and a taskbar, and a
/// window called `title`, on screen. Whether it has the keys is the test's
/// to ask.
fn program(
    session: &Session,
    title: &str,
) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList, Ear) {
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, title, title);
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), title).is_some()
    });
    (desk, queue, qh, globals, ear)
}

/// Whether this program's `nth` window has the keys, as its keyboard last
/// heard.
fn has_keys(desk: &Desk, ear: &Ear, nth: usize) -> bool {
    heard(ear).entered.as_ref() == Some(&desk.windows[nth].wl_surface().id())
}

/// The terminal the person types at: the first program, whose window has
/// the keys, nothing having had them before.
fn terminal(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, GlobalList, Ear) {
    let (mut desk, mut queue, qh, globals, ear) = program(session, "terminal");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));
    (desk, queue, qh, globals, ear)
}

/// The person types `text` at whatever has the keys, and it is taken in
/// before this returns: a roundtrip of the program they type at, whose keys
/// are sent before its answer.
fn types(session: &Session, desk: &mut Desk, queue: &mut EventQueue<Desk>, text: &str) {
    session.command(Do::Type(text.to_owned()));
    queue.roundtrip(desk).expect("dispatch");
    queue.roundtrip(desk).expect("dispatch");
}

/// The issue's own case. The person types at a terminal, and another
/// program opens a window. It opens without the keys: the terminal keeps
/// them, the window is not told it is active, and it is beneath the
/// terminal, not over it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_another_program_opens_while_the_person_types_opens_without_the_keys() {
    let session = seated("withheld-another");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    types(&session, &mut desk, &mut queue, "sudo");

    let (mut thief, mut thief_queue, _, _, thief_ear) = program(&session, "thief");
    until(&mut thief_queue, &mut thief, |thief| thief.has_drawn(0));
    queue.roundtrip(&mut desk).expect("dispatch");
    thief_queue.roundtrip(&mut thief).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(heard(&thief_ear).entered.is_none(), "the thief has none");
    assert!(
        !thief.activations_of(0).contains(&true),
        "nor is it told it is active: {:?}",
        thief.activations_of(0)
    );
    let facts = session.facts.read();
    assert_eq!(stack(&facts), ["thief", "terminal"], "beneath the terminal");
    assert!(!given_keys(&facts, "thief"));
    drop(facts);

    session.stop(((desk, queue), (thief, thief_queue)));
}

/// Beneath the window in use, not beneath every window: in sight, just
/// behind what the person is typing into.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opens_without_the_keys_goes_just_beneath_the_window_in_use() {
    let session = seated("withheld-beneath");
    let (mut desk, mut queue, qh, _, ear) = terminal(&session);
    desk.open_window(&qh, "editor", "editor");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 1));
    desk.activate("terminal");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));
    session.wait_for(|facts| stack(facts) == ["editor", "terminal"]);

    let (mut thief, mut thief_queue, _, _, _) = program(&session, "thief");
    until(&mut thief_queue, &mut thief, |thief| thief.has_drawn(0));
    session.wait_for(|facts| stack(facts) == ["editor", "thief", "terminal"]);

    session.stop(((desk, queue), (thief, thief_queue)));
}

/// A dialog of the program in use, or another of its windows: the person
/// is behind what the program they are using asks for.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_of_the_program_in_use_opens_with_the_keys() {
    let session = seated("withheld-own");
    let (mut desk, mut queue, qh, _, ear) = terminal(&session);
    desk.open_dialog(&qh, "save as", "terminal", 0);
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 1));
    session.wait_for(|facts| on_top(facts).as_deref() == Some("save as"));
    session.stop((desk, queue));
}

/// The person left for an empty workspace, and nothing has the keys: a
/// window opening there has them, as anything does where nothing else is.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_opening_where_nothing_has_the_keys_takes_them() {
    use perspicax_compositor::Virtual;
    use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape};

    let session = Session::start(
        "withheld-nothing",
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (1280, 800))],
            workspaces: Shape {
                mode: Mode::Spanning,
                grid: Grid {
                    columns: 2,
                    rows: 1,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: true,
            opacity: Default::default(),
            agent: Default::default(),
        },
    );
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    session.perform(Action::Workspace(Direction::Right));
    until(&mut queue, &mut desk, |_| heard(&ear).entered.is_none());

    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });

    session.stop(((desk, queue), (other, other_queue)));
}

/// With nobody at the seat, nobody is typing anything to take: every
/// window takes the keys as it opens, as M2's contract has it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_nobody_at_the_seat_every_window_opens_with_the_keys() {
    let session = Session::start("withheld-nobody", Backend::headless((1280, 800)));
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    session.stop(((desk, queue), (other, other_queue)));
}

/// A window its program unmapped, as xdg-shell lets it, and maps again
/// while another program is in use, comes back as a new window does:
/// without the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_its_program_maps_again_while_another_is_in_use_opens_without_the_keys() {
    let session = seated("withheld-again");
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    let (mut desk, mut queue, _, _, ear) = program(&session, "terminal");
    desk.activate("terminal");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));

    let surface = other.windows[0].wl_surface().clone();
    surface.attach(None, 0, 0);
    surface.commit();
    other_queue.roundtrip(&mut other).expect("dispatch");
    session.wait_for(|facts| titled(facts, "other").is_none());
    // Asked for again, then drawn: a new window's steps.
    other.windows[0].commit();
    until(&mut other_queue, &mut other, |other| {
        other.has_drawn(0) && titled(&session.facts.read(), "other").is_some()
    });
    other_queue.roundtrip(&mut other).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(!has_keys(&other, &other_ear, 0), "the window back has none");
    assert_eq!(on_top(&session.facts.read()).as_deref(), Some("terminal"));

    session.stop(((desk, queue), (other, other_queue)));
}

/// A window that opened without the keys has them once the program in use
/// brings it forward with a token, as a link opened in another program's
/// window does: a token is the other way of asking.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opened_without_the_keys_takes_them_with_a_token() {
    let session = seated("withheld-token");
    let (mut desk, mut queue, qh, globals, ear) = terminal(&session);
    desk.bind_activation(&globals, &qh);
    let (mut other, mut other_queue, other_qh, other_globals, other_ear) =
        program(&session, "other");
    other.bind_activation(&other_globals, &other_qh);
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));

    let serial = heard(&ear).enter_serial.expect("entered");
    desk.make_token(&qh, 0, serial);
    until(&mut queue, &mut desk, |desk| !desk.tokens.is_empty());
    let token = desk.tokens.remove(0);
    other.spend_token(&token, 0);
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    session.wait_for(|facts| on_top(facts).as_deref() == Some("other"));

    session.stop(((desk, queue), (other, other_queue)));
}

/// The window in use closes, and the keys go back to the window beneath
/// it, passing over one that opened without them, which the person never
/// gave them to. The window they go to is raised over it, so they go where
/// the person can see.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn when_the_window_in_use_closes_the_keys_pass_over_a_window_that_opened_without_them() {
    let session = seated("withheld-closes");
    let (mut desk, mut queue, qh, _, ear) = terminal(&session);
    desk.open_window(&qh, "editor", "editor");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 1));
    let (mut thief, mut thief_queue, _, _, thief_ear) = program(&session, "thief");
    session.wait_for(|facts| stack(facts) == ["terminal", "thief", "editor"]);

    desk.windows.remove(1);
    queue.roundtrip(&mut desk).expect("dispatch");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));
    session.wait_for(|facts| stack(facts) == ["thief", "terminal"]);
    thief_queue.roundtrip(&mut thief).expect("dispatch");
    assert!(heard(&thief_ear).entered.is_none(), "the thief has none");

    session.stop(((desk, queue), (thief, thief_queue)));
}

/// Unless it is the only window left: then nothing else is there to have
/// them, and it does.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn when_the_window_in_use_closes_the_only_window_left_takes_the_keys() {
    let session = seated("withheld-only");
    let (mut desk, mut queue, _, _, _) = terminal(&session);
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));

    desk.windows.remove(0);
    queue.roundtrip(&mut desk).expect("dispatch");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });

    session.stop(((desk, queue), (other, other_queue)));
}

/// A program's next window opens where its last one closed, at its first
/// picture, which moves it from where the cascade put it: one that opened
/// without the keys stays beneath the window in use as it goes there.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opened_without_the_keys_stays_beneath_as_it_goes_where_its_program_was() {
    let session = seated("withheld-remembered");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut other, mut other_queue, other_qh, _, _) = program(&session, "other");
    let was = session
        .wait_for(|facts| titled(facts, "other").is_some_and(drawn))
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some("other"))
        .map(|surface| (surface.geometry.x0, surface.geometry.y0))
        .expect("waited for");
    other.windows.remove(0);
    other_queue.roundtrip(&mut other).expect("dispatch");
    session.wait_for(|facts| titled(facts, "other").is_none());

    other.open_window(&other_qh, "other again", "other");
    until(&mut other_queue, &mut other, |_| {
        titled(&session.facts.read(), "other again")
            .is_some_and(|window| drawn(window) && (window.geometry.x0, window.geometry.y0) == was)
    });
    assert_eq!(stack(&session.facts.read()), ["other again", "terminal"]);
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0));

    session.stop(((desk, queue), (other, other_queue)));
}

/// Whether a window has drawn, so has a size.
fn drawn(window: &SurfaceFacts) -> bool {
    window.geometry.x1 > window.geometry.x0
}

/// What [`window_probe`] does, by [`STEPS`].
#[derive(Clone, Copy)]
enum Steps {
    /// Open its window at once.
    Window,
    /// Wait for the gate before it so much as connects: a program that was
    /// already running, left in the background.
    WaitThenWindow,
    /// Connect, then wait for the gate before opening its window.
    ConnectWaitWindow,
    /// Open a splash, close it once it has drawn, then open its window.
    Splash,
}

impl Steps {
    fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::WaitThenWindow => "wait-then-window",
            Self::ConnectWaitWindow => "connect-wait-window",
            Self::Splash => "splash",
        }
    }
}

/// A program in a process of its own, as a terminal starts one: a copy of
/// this binary running [`window_probe`], its window titled "probe". Killed
/// and reaped as it is dropped, whether its test passes or not.
struct Probe {
    child: Child,
    /// The file it waits for before its next step, where it has one.
    gate: PathBuf,
}

impl Probe {
    fn start(session: &Session, steps: Steps) -> Self {
        // Beside the session's socket, and named for it: never the socket.
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let gate = Path::new(&dir).join(format!("{}.gate", session.socket()));
        let _ = std::fs::remove_file(&gate);
        let _ = std::fs::remove_file(connected(&gate));
        // Its panics on stderr, beside this test's; its harness's report
        // nowhere.
        let child = Command::new(std::env::current_exe().expect("this test's binary"))
            .args(["--exact", "window_probe", "--ignored", "--nocapture"])
            .env(PROBE, session.socket())
            .env(STEPS, steps.name())
            .env(GATE, &gate)
            .stdout(Stdio::null())
            .spawn()
            .expect("the probe starts");
        Self { child, gate }
    }

    /// Let it take its next step.
    fn open_gate(&self) {
        std::fs::write(&self.gate, b"").expect("the gate opens");
    }

    /// Wait until it says it is connected.
    fn connected(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !connected(&self.gate).exists() {
            assert!(Instant::now() < deadline, "the probe never connected");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait for its window, and say whether it was given the keys.
    fn given_keys(&self, session: &Session) -> bool {
        let facts = session.wait_for(|facts| titled(facts, "probe").is_some());
        given_keys(&facts, "probe")
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(connected(&self.gate));
        let _ = std::fs::remove_file(&self.gate);
    }
}

/// The file a probe that waits once connected leaves beside its gate, to
/// say it is.
fn connected(gate: &Path) -> PathBuf {
    gate.with_extension("up")
}

/// The person runs a program at the terminal, with an Enter: its window
/// opens with the keys, the program having started since, under the
/// terminal.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_started_by_an_enter_at_the_window_in_use_opens_with_the_keys() {
    let session = seated("withheld-enter");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    types(&session, &mut desk, &mut queue, "gimp\n");

    let probe = Probe::start(&session, Steps::Window);
    assert!(probe.given_keys(&session), "the program run has the keys");
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    drop(probe);
    session.stop((desk, queue));
}

/// A program the terminal started before the Enter, still running in the
/// background, opens a window just after it: the Enter did not start it,
/// and it opens without the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_started_before_the_enter_opens_without_the_keys() {
    let session = seated("withheld-before");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let probe = Probe::start(&session, Steps::WaitThenWindow);
    // Long enough for the probe's start to be a tick or more before the
    // Enter's: a start is told in hundredths of a second.
    std::thread::sleep(Duration::from_millis(50));
    types(&session, &mut desk, &mut queue, "sudo true\n");

    probe.open_gate();
    assert!(
        !probe.given_keys(&session),
        "the background program has none"
    );
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    drop(probe);
    session.stop((desk, queue));
}

/// Only an Enter starts what was typed. A program that starts as the person
/// types a letter -- a password, say -- opens without the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_started_as_the_person_types_a_letter_opens_without_the_keys() {
    let session = seated("withheld-letter");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    types(&session, &mut desk, &mut queue, "x");

    let probe = Probe::start(&session, Steps::Window);
    assert!(!probe.given_keys(&session), "it has none");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    drop(probe);
    session.stop((desk, queue));
}

/// The program the person ran is slow to open its window, and they type
/// something meanwhile: by then they have moved on, and it opens without
/// the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_the_person_has_typed_past_opens_without_the_keys() {
    let session = seated("withheld-past");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    types(&session, &mut desk, &mut queue, "gimp\n");
    let probe = Probe::start(&session, Steps::ConnectWaitWindow);
    // Its connection is accepted, stamped with the Enter, before the letter.
    probe.connected();
    types(&session, &mut desk, &mut queue, "l");

    probe.open_gate();
    assert!(!probe.given_keys(&session), "it has none");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    drop(probe);
    session.stop((desk, queue));
}

/// A program the person ran shows a splash, which takes the keys, and
/// closes it before its window opens: the keys go back to the terminal
/// meanwhile, but the person pressed nothing, and the window opens with
/// them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_programs_window_opens_with_the_keys_after_its_splash_has_closed() {
    let session = seated("withheld-splash");
    let (mut desk, mut queue, _, _, _) = terminal(&session);
    types(&session, &mut desk, &mut queue, "gimp\n");

    let probe = Probe::start(&session, Steps::Splash);
    assert!(probe.given_keys(&session), "its window has the keys");
    drop(probe);
    session.stop((desk, queue));
}

/// Not a test: the program a [`Probe`] is, in a process of its own.
#[test]
#[ignore = "a program in a process of its own, for the tests that start one"]
fn window_probe() {
    let Ok(socket) = std::env::var(PROBE) else {
        return;
    };
    let steps = std::env::var(STEPS).unwrap_or_default();
    let gate = PathBuf::from(std::env::var_os(GATE).unwrap_or_default());
    let wait = || {
        while !gate.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    if steps == Steps::WaitThenWindow.name() {
        wait();
    }
    let (mut desk, mut queue, qh, _) = common::client_on(&socket);
    queue.roundtrip(&mut desk).expect("the compositor is there");
    if steps == Steps::ConnectWaitWindow.name() {
        std::fs::write(connected(&gate), b"").expect("said to be connected");
        wait();
    }
    if steps == Steps::Splash.name() {
        desk.open_window(&qh, "splash", "probe");
        until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
        desk.windows.remove(0);
        queue.roundtrip(&mut desk).expect("the splash closed");
    }
    desk.open_window(&qh, "probe", "probe");
    loop {
        queue
            .blocking_dispatch(&mut desk)
            .expect("the compositor is there");
    }
}
