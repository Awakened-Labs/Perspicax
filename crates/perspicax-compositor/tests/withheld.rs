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
//! before. A shell is told, from version 7 of its channel, that such a
//! window wants attention, and so is one whose program asked for the keys
//! and was refused, until the person gives it them. What a binding of the
//! person's starts is the root of their press, and is handed a token of
//! the compositor's own, good while that press is their latest.
//!
//! A launcher, a layer over the windows that asks for every key as fuzzel
//! does, is asked the same as it comes to want them (#267), but never for
//! nothing having the keys: one nobody asked for is shown without them.
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
use perspicax_index::{HostFacts, SurfaceFacts, SurfaceKind};
use perspicax_protocols::shell::v1::client::perspicax_shell_v1::WindowState;
use smithay_client_toolkit::shell::{WaylandSurface as _, wlr_layer::KeyboardInteractivity};
use wayland_client::{EventQueue, Proxy as _, QueueHandle, backend::ObjectId, globals::GlobalList};

/// Set, to the compositor's socket, for the copy of this binary that
/// [`window_probe`] runs in.
const PROBE: &str = "PERSPICAX_WITHHELD_PROBE";
/// What the probe does, and in what order: see [`Steps`].
const STEPS: &str = "PERSPICAX_WITHHELD_STEPS";
/// A file the probe waits for before its next step, where it has one.
const GATE: &str = "PERSPICAX_WITHHELD_GATE";
/// How tall a launcher is, across the top of the screen.
const STRIP: u32 = 120;
const LAUNCHER: u32 = 0xff44_4444;

/// A session with a person at the seat.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

/// A session with a person at the seat, and two workspaces side by side.
fn seated_on_two_workspaces(name: &str) -> Session {
    use perspicax_compositor::Virtual;
    use perspicax_policy::{Access, Grid, Mode, Shape};

    Session::start(
        name,
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
    )
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
    use perspicax_policy::{Action, Direction};

    let session = seated_on_two_workspaces("withheld-nothing");
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
    let (mut panel, mut panel_queue) = panel(&session, 7);
    pick(&session, &mut panel, &mut panel_queue, "terminal");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));
    // Told it is no longer active, and drawn so, before it goes: a window
    // draws at every configure, which would map it again.
    until(&mut other_queue, &mut other, |_| {
        heard(&other_ear).entered.is_none()
    });

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

    session.stop(((desk, queue), (other, other_queue), (panel, panel_queue)));
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

/// The person's panel: a client of its own with the shell's channel and a
/// taskbar, bound as perspicax-shell binds them, at `version`, and no
/// window.
fn panel(session: &Session, version: u32) -> (Desk, EventQueue<Desk>) {
    let (mut panel, mut queue, qh, globals) = session.client();
    match version {
        7 => panel.bind_shell(&globals, &qh),
        _ => panel.bind_shell_v6(&globals, &qh),
    }
    panel.bind_taskbar(&globals, &qh);
    queue.roundtrip(&mut panel).expect("bound");
    (panel, queue)
}

/// The person picks the window `title` in the panel's taskbar: their press
/// reaches the panel, and it asks for the window (#270).
fn pick(session: &Session, panel: &mut Desk, queue: &mut EventQueue<Desk>, title: &str) {
    until(queue, panel, |panel| panel.task(title).is_some());
    common::press_reaching(session, panel, queue);
    panel.activate(title);
    queue.roundtrip(panel).expect("the pick sent");
}

/// Whether the panel was last told the window `title` wants attention, once
/// it has been told of it at all.
fn told_wanting(panel: &mut Desk, queue: &mut EventQueue<Desk>, title: &str) -> bool {
    until(queue, panel, |panel| panel.told_of(title).0.is_some());
    // Told at the end of a turn of the loop, after the answer to a round
    // trip in it: one more, for what this turn changed.
    queue.roundtrip(panel).expect("dispatch");
    queue.roundtrip(panel).expect("dispatch");
    panel
        .told_of(title)
        .0
        .is_some_and(|told| told.state.contains(WindowState::WantsAttention))
}

/// Wait for the panel to be told the window `title` wants attention, or no
/// longer does.
fn told_so(panel: &mut Desk, queue: &mut EventQueue<Desk>, title: &str, wanting: bool) {
    until(queue, panel, |panel| {
        panel
            .told_of(title)
            .0
            .is_some_and(|told| told.state.contains(WindowState::WantsAttention) == wanting)
    });
}

/// The panel is told that a window that opened without the keys wants
/// attention, and that the window in use does not. The person picks it
/// there, and it wants it no longer.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_that_opened_without_the_keys_wants_attention_until_the_person_picks_it() {
    let session = seated("withheld-wants");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut panel, mut panel_queue) = panel(&session, 7);
    let (mut thief, mut thief_queue, _, _, thief_ear) = program(&session, "thief");

    told_so(&mut panel, &mut panel_queue, "thief", true);
    assert!(!told_wanting(&mut panel, &mut panel_queue, "terminal"));
    pick(&session, &mut panel, &mut panel_queue, "thief");
    until(&mut thief_queue, &mut thief, |thief| {
        has_keys(thief, &thief_ear, 0)
    });
    told_so(&mut panel, &mut panel_queue, "thief", false);
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(!has_keys(&desk, &ear, 0));

    session.stop(((desk, queue), (thief, thief_queue), (panel, panel_queue)));
}

/// A shell bound at version 6 knows no such bit, and is never sent it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_shell_bound_at_version_6_is_never_told_a_window_wants_attention() {
    let session = seated("withheld-v6");
    let (desk, queue, _, _, _) = terminal(&session);
    let (mut panel, mut panel_queue) = panel(&session, 6);
    let (thief, thief_queue, _, _, _) = program(&session, "thief");
    session.wait_for(|facts| titled(facts, "thief").is_some_and(drawn));

    assert!(!told_wanting(&mut panel, &mut panel_queue, "thief"));

    session.stop(((desk, queue), (thief, thief_queue), (panel, panel_queue)));
}

/// A program the person has moved on from asks to bring its window forward
/// with a token made before: refused, and the window wants attention.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_refused_the_keys_by_a_stale_token_wants_attention() {
    let session = seated("withheld-stale");
    let (mut other, mut other_queue, other_qh, other_globals, other_ear) =
        program(&session, "other");
    other.bind_activation(&other_globals, &other_qh);
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    let serial = heard(&other_ear).enter_serial.expect("entered");
    other.make_token(&other_qh, 0, serial);
    until(&mut other_queue, &mut other, |other| {
        !other.tokens.is_empty()
    });
    let token = other.tokens.remove(0);
    let (mut panel, mut panel_queue) = panel(&session, 7);
    let (mut desk, mut queue, _, _, ear) = program(&session, "terminal");
    pick(&session, &mut panel, &mut panel_queue, "terminal");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));
    assert!(!told_wanting(&mut panel, &mut panel_queue, "other"));

    other.spend_token(&token, 0);
    other_queue.roundtrip(&mut other).expect("dispatch");
    told_so(&mut panel, &mut panel_queue, "other", true);
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");

    session.stop(((desk, queue), (other, other_queue), (panel, panel_queue)));
}

/// Unless it is the window in use: refused what it has already, it wants
/// nothing.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_window_in_use_refused_the_keys_by_a_stale_token_wants_no_attention() {
    let session = seated("withheld-in-use");
    let (mut desk, mut queue, qh, globals, ear) = terminal(&session);
    desk.bind_activation(&globals, &qh);
    let serial = heard(&ear).enter_serial.expect("entered");
    desk.make_token(&qh, 0, serial);
    until(&mut queue, &mut desk, |desk| !desk.tokens.is_empty());
    let token = desk.tokens.remove(0);
    let (mut panel, mut panel_queue) = panel(&session, 7);
    types(&session, &mut desk, &mut queue, "x");

    desk.spend_token(&token, 0);
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(!told_wanting(&mut panel, &mut panel_queue, "terminal"));

    session.stop(((desk, queue), (panel, panel_queue)));
}

/// A binding of the person's: a press of the side button on the desktop
/// starts `command`, for them.
#[cfg(feature = "capture")]
fn bind_spawn(session: &Session, command: Vec<String>) {
    bind_side(
        session,
        perspicax_policy::Action::Spawn {
            command,
            agent: false,
        },
    );
}

/// A binding of the person's: a press of the side button on the desktop
/// does `action`.
#[cfg(feature = "capture")]
fn bind_side(session: &Session, action: perspicax_policy::Action) {
    use perspicax_policy::{Button, Context, Gesture, Mods, MouseBindings, MouseChord};
    session.command(Do::MouseBindings(MouseBindings::default().bind(
        Context::Desktop,
        MouseChord {
            mods: Mods::default(),
            gesture: Gesture::Press(Button::Side),
        },
        action,
    )));
}

/// The person presses the binding, on the desktop's far corner, where no
/// window is.
#[cfg(feature = "capture")]
fn press_binding(session: &Session) {
    use perspicax_policy::{Button, Mods};
    session.command(Do::Click {
        at: (1270, 790),
        button: Button::Side,
        mods: Mods::default(),
    });
}

/// What the binding started was handed, as `XDG_ACTIVATION_TOKEN` and
/// `DESKTOP_STARTUP_ID`: bound, pressed, and written down by a shell it
/// starts.
#[cfg(feature = "capture")]
fn handed(session: &Session) -> (String, String) {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let said = Path::new(&dir).join(format!("{}.handed", session.socket()));
    let _ = std::fs::remove_file(&said);
    let script = r#"printf '%s\n%s' "$XDG_ACTIVATION_TOKEN" "$DESKTOP_STARTUP_ID" > "$0.part" && mv "$0.part" "$0""#;
    bind_spawn(
        session,
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            script.to_owned(),
            said.to_string_lossy().into_owned(),
        ],
    );
    press_binding(session);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !said.exists() {
        assert!(Instant::now() < deadline, "the binding started nothing");
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = std::fs::read_to_string(&said).expect("written down");
    let _ = std::fs::remove_file(&said);
    let (token, startup) = text.split_once('\n').expect("two lines");
    (token.to_owned(), startup.to_owned())
}

/// The person's binding starts a program: the root of their press, its
/// window opens with the keys, though the terminal had them.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_a_binding_of_the_persons_starts_opens_with_the_keys() {
    let session = seated("withheld-binding");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    // The probe, in the binding's own process, its harness's report nowhere.
    let script = format!(
        "{PROBE}=\"$0\" {STEPS}={} exec \"$1\" --exact window_probe --ignored --nocapture > /dev/null",
        Steps::Window.name()
    );
    let exe = std::env::current_exe().expect("this test's binary");
    bind_spawn(
        &session,
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            script,
            session.socket().to_owned(),
            exe.to_string_lossy().into_owned(),
        ],
    );
    press_binding(&session);

    let facts = session.wait_for(|facts| titled(facts, "probe").is_some());
    assert!(
        given_keys(&facts, "probe"),
        "the program started has the keys"
    );
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    session.stop((desk, queue));
}

/// What the binding starts is handed a token of the compositor's own, under
/// both names: a program it hands on to, already running, brings its window
/// forward with it, as a browser started again opens its window in the one
/// running.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_a_binding_starts_is_handed_a_token_that_brings_a_window_forward() {
    let session = seated("withheld-handed");
    let (desk, queue, _, _, _) = terminal(&session);
    let (mut other, mut other_queue, other_qh, other_globals, other_ear) =
        program(&session, "other");
    other.bind_activation(&other_globals, &other_qh);
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));

    let (token, startup) = handed(&session);
    assert!(!token.is_empty(), "handed a token");
    assert_eq!(startup, token, "and the same as its startup id, for X11");
    other.spend_token(&token, 0);
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });

    session.stop(((desk, queue), (other, other_queue)));
}

/// But only while the binding's press is the person's latest: a key pressed
/// since, and it brings nothing forward.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_a_binding_hands_on_is_good_until_the_person_presses_something_else() {
    let session = seated("withheld-handed-since");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut other, mut other_queue, other_qh, other_globals, other_ear) =
        program(&session, "other");
    other.bind_activation(&other_globals, &other_qh);
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));

    let (token, _) = handed(&session);
    types(&session, &mut desk, &mut queue, "x");
    other.spend_token(&token, 0);
    other_queue.roundtrip(&mut other).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(!has_keys(&other, &other_ear, 0));

    session.stop(((desk, queue), (other, other_queue)));
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
    /// Show a launcher, as fuzzel does, and no window.
    Launcher,
}

impl Steps {
    fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::WaitThenWindow => "wait-then-window",
            Self::ConnectWaitWindow => "connect-wait-window",
            Self::Splash => "splash",
            Self::Launcher => "launcher",
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

/// A program with a keyboard and nothing on screen yet: a client of its
/// own.
fn client(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, Ear) {
    let (mut desk, queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    (desk, queue, qh, ear)
}

/// Show a launcher on `desk`'s client, as fuzzel shows one: a strip over
/// everything, asking for every key, drawn. Its commits are taken in, and
/// whatever came of them sent, before this returns. Its surface.
fn show_launcher(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
) -> ObjectId {
    let before = desk.layers_drawn;
    desk.open_launcher(qh, STRIP, LAUNCHER);
    let surface = desk.layers.last().expect("just opened").0.wl_surface().id();
    until(queue, desk, |desk| desk.layers_drawn > before);
    surface
}

/// Whether a launcher is on screen, as the facts have it.
fn launcher_shown(facts: &HostFacts) -> bool {
    facts.surfaces().iter().any(|surface| {
        surface.mapped
            && matches!(&surface.kind, SurfaceKind::Layer { namespace, .. } if namespace == "launcher")
    })
}

/// The issue's own case (#267): the person types at a terminal, and another
/// program shows a launcher. It is on screen, over everything, but takes
/// none of the keys: the rest of what the person types goes to the
/// terminal still.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_another_program_shows_while_the_person_types_takes_no_keys() {
    let session = seated("withheld-launcher");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    types(&session, &mut desk, &mut queue, "sudo");

    let (mut thief, mut thief_queue, thief_qh, thief_ear) = client(&session);
    show_launcher(&mut thief, &mut thief_queue, &thief_qh);
    session.wait_for(launcher_shown);
    let before = heard(&ear).keys.len();
    types(&session, &mut desk, &mut queue, "x");
    thief_queue.roundtrip(&mut thief).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(heard(&ear).keys.len() > before, "and hears the rest");
    assert!(heard(&thief_ear).entered.is_none(), "the launcher has none");
    assert!(heard(&thief_ear).keys.is_empty(), "nor hears any");

    session.stop(((desk, queue), (thief, thief_queue)));
}

/// A launcher of the program in use, a run box the terminal opened say,
/// takes the keys: the person is behind what the program they use asks for.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_of_the_program_in_use_takes_the_keys() {
    let session = seated("withheld-launcher-own");
    let (mut desk, mut queue, qh, _, ear) = terminal(&session);
    let launcher = show_launcher(&mut desk, &mut queue, &qh);
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&launcher)
    });
    session.stop((desk, queue));
}

/// Nothing has the keys -- the person is at an empty desk -- and a program
/// shows a launcher: it takes none of them, though a window would. A
/// launcher keeps them from every window while it is up (#131), so the
/// terminal the person opened next would have none.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_shown_where_nothing_has_the_keys_takes_none() {
    let session = seated("withheld-launcher-nothing");
    let (mut thief, mut thief_queue, thief_qh, thief_ear) = client(&session);
    show_launcher(&mut thief, &mut thief_queue, &thief_qh);
    session.wait_for(launcher_shown);
    assert!(heard(&thief_ear).entered.is_none(), "the launcher has none");

    // Waits until the terminal has the keys, and fails if it never does.
    let (desk, queue, _, _, _) = terminal(&session);
    session.stop(((desk, queue), (thief, thief_queue)));
}

/// A launcher shown without the keys is not asked again as it draws. Its
/// program comes to have them, its window picked in the taskbar, and the
/// launcher draws again: it has none still, for the person picked the
/// window. It is asked again only as it next comes to want every key, and
/// then, its program being in use, it takes them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_refused_the_keys_is_asked_again_only_as_it_next_wants_them() {
    let session = seated("withheld-launcher-again");
    let (desk, queue, _, _, _) = terminal(&session);
    let (mut panel, mut panel_queue) = panel(&session, 7);
    let (mut other, mut other_queue, other_qh, _, other_ear) = program(&session, "other");
    until(&mut other_queue, &mut other, |other| other.has_drawn(0));
    let launcher = show_launcher(&mut other, &mut other_queue, &other_qh);
    assert!(heard(&other_ear).entered.is_none(), "neither has the keys");

    pick(&session, &mut panel, &mut panel_queue, "other");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    other.draw_layer(0);
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        has_keys(&other, &other_ear, 0),
        "the window picked keeps them as the launcher draws"
    );

    let layer = other.layers[0].0.clone();
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(
        has_keys(&other, &other_ear, 0),
        "wanting none, it takes none"
    );
    layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer.commit();
    until(&mut other_queue, &mut other, |_| {
        heard(&other_ear).entered.as_ref() == Some(&launcher)
    });

    session.stop(((desk, queue), (other, other_queue), (panel, panel_queue)));
}

/// fuzzel, from a binding of the person's: what the binding started is the
/// root of their press, and its launcher takes the keys though the terminal
/// had them.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_a_binding_of_the_persons_starts_takes_the_keys() {
    let session = seated("withheld-launcher-binding");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    // The probe, in the binding's own process, its harness's report nowhere.
    let script = format!(
        "{PROBE}=\"$0\" {STEPS}={} exec \"$1\" --exact window_probe --ignored --nocapture > /dev/null",
        Steps::Launcher.name()
    );
    let exe = std::env::current_exe().expect("this test's binary");
    bind_spawn(
        &session,
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            script,
            session.socket().to_owned(),
            exe.to_string_lossy().into_owned(),
        ],
    );
    press_binding(&session);

    session.wait_for(launcher_shown);
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    session.stop((desk, queue));
}

/// A launcher shown without the keys has them once the person clicks it,
/// as a panel that takes them on a click does: what they click on is what
/// they asked for.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launcher_shown_without_the_keys_takes_them_when_the_person_clicks_it() {
    use perspicax_policy::{Button, Mods};

    let session = seated("withheld-launcher-clicked");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut other, mut other_queue, other_qh, other_ear) = client(&session);
    let launcher = show_launcher(&mut other, &mut other_queue, &other_qh);
    session.wait_for(launcher_shown);
    assert!(
        heard(&other_ear).entered.is_none(),
        "shown without the keys"
    );

    // In the strip, across the top of the screen.
    session.command(Do::Click {
        at: (640, 60),
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut other_queue, &mut other, |_| {
        heard(&other_ear).entered.as_ref() == Some(&launcher)
    });
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    session.stop(((desk, queue), (other, other_queue)));
}

/// The person's binding asks the shell for its start menu. The binding
/// took their press, and hands it on to the shell, so the menu the shell
/// shows in answer, a launcher, takes the keys though the terminal had them.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_a_binding_asks_the_shell_for_takes_the_keys() {
    use common::Told;

    let session = seated("withheld-launcher-shell");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    // The shell, on a connection of its own, as perspicax-shell is.
    let (mut shell, mut shell_queue, shell_qh, shell_globals) = session.client();
    shell.bind_shell(&shell_globals, &shell_qh);
    let shell_ear = shell.bind_keyboard(&shell_globals, &shell_qh);
    shell_queue.roundtrip(&mut shell).expect("bound");
    bind_side(&session, perspicax_policy::Action::StartMenu);
    press_binding(&session);
    until(&mut shell_queue, &mut shell, |shell| {
        shell
            .told
            .iter()
            .any(|told| matches!(told, Told::StartMenu(_)))
    });

    let menu = show_launcher(&mut shell, &mut shell_queue, &shell_qh);
    until(&mut shell_queue, &mut shell, |_| {
        heard(&shell_ear).entered.as_ref() == Some(&menu)
    });
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    session.stop(((desk, queue), (shell, shell_queue)));
}

/// A program with a taskbar and nothing else: no window of its own, and
/// none of the person's presses reach it. What it asks of a window, it asks
/// for itself.
fn background_taskbar(session: &Session) -> (Desk, EventQueue<Desk>) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    queue.roundtrip(&mut desk).expect("bound");
    (desk, queue)
}

/// The person at the terminal, picked in the panel over the window of
/// another program, "other", which had the keys before it: the window the
/// keys would go to, were the terminal to go, and one wanting no attention.
#[expect(
    clippy::type_complexity,
    reason = "three clients, as the tests take them"
)]
fn terminal_over_other(
    session: &Session,
) -> (
    (Desk, EventQueue<Desk>, Ear),
    (Desk, EventQueue<Desk>, Ear),
    (Desk, EventQueue<Desk>),
) {
    let (mut other, mut other_queue, _, _, other_ear) = program(session, "other");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    let (mut desk, mut queue, _, _, ear) = program(session, "terminal");
    let (mut panel, mut panel_queue) = panel(session, 7);
    pick(session, &mut panel, &mut panel_queue, "terminal");
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear, 0));
    session.wait_for(|facts| stack(facts) == ["other", "terminal"]);
    assert!(!told_wanting(&mut panel, &mut panel_queue, "other"));
    (
        (desk, queue, ear),
        (other, other_queue, other_ear),
        (panel, panel_queue),
    )
}

/// The issue's own case (#270): the person types at a terminal, and another
/// program asks its taskbar to bring its own window forward, as any program
/// admitted to foreign-toplevel-management may. Nobody asked it to: the
/// terminal keeps the keys, the window stays beneath it, and its tab wants
/// attention, as one whose token was refused does.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_that_brings_its_own_window_forward_while_the_person_types_takes_no_keys() {
    let session = seated("withheld-taskbar");
    let ((mut desk, mut queue, ear), (mut other, mut other_queue, other_ear), mut panel) =
        terminal_over_other(&session);
    types(&session, &mut desk, &mut queue, "sudo");

    other.activate("other");
    other_queue.roundtrip(&mut other).expect("the request sent");
    told_so(&mut panel.0, &mut panel.1, "other", true);
    queue.roundtrip(&mut desk).expect("dispatch");
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(heard(&other_ear).entered.is_none(), "the program has none");
    assert_eq!(
        stack(&session.facts.read()),
        ["other", "terminal"],
        "beneath the terminal"
    );

    session.stop(((desk, queue), (other, other_queue), panel));
}

/// Nor may a program bring another program's window forward, nor mark it:
/// no program says a window not its own wants attention, which would change
/// which window the keys go back to.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_that_brings_another_programs_window_forward_moves_and_marks_nothing() {
    let session = seated("withheld-taskbar-another");
    let ((mut desk, mut queue, ear), other, mut panel) = terminal_over_other(&session);
    let (mut thief, mut thief_queue) = background_taskbar(&session);

    until(&mut thief_queue, &mut thief, |thief| {
        thief.task("other").is_some()
    });
    thief.activate("other");
    thief_queue.roundtrip(&mut thief).expect("the request sent");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(
        !told_wanting(&mut panel.0, &mut panel.1, "other"),
        "and the window is marked by nobody"
    );
    assert_eq!(stack(&session.facts.read()), ["other", "terminal"]);

    session.stop((
        (desk, queue),
        (other.0, other.1),
        panel,
        (thief, thief_queue),
    ));
}

/// Nor minimize the window in use, nor close it: either hands the keys to
/// the window beneath, and the rest of a password with them.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_can_neither_minimize_nor_close_the_window_in_use() {
    use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

    let session = seated("withheld-taskbar-minimize");
    let ((mut desk, mut queue, ear), (mut other, mut other_queue, other_ear), panel) =
        terminal_over_other(&session);
    let (mut thief, mut thief_queue) = background_taskbar(&session);

    until(&mut thief_queue, &mut thief, |thief| {
        thief.task("terminal").is_some()
    });
    let terminal = thief.task("terminal").expect("listed").handle.clone();
    terminal.set_minimized();
    terminal.close();
    thief_queue
        .roundtrip(&mut thief)
        .expect("the requests sent");
    queue.roundtrip(&mut desk).expect("dispatch");
    other_queue.roundtrip(&mut other).expect("dispatch");
    thief_queue.roundtrip(&mut thief).expect("dispatch");
    assert!(has_keys(&desk, &ear, 0), "the terminal keeps the keys");
    assert!(
        heard(&other_ear).entered.is_none(),
        "and nothing beneath has any"
    );
    assert_eq!(desk.asked_to_close, 0, "nor is the terminal asked to close");
    assert!(
        !thief.task("terminal").expect("listed").is(State::Minimized),
        "nor minimized"
    );

    session.stop((
        (desk, queue),
        (other, other_queue),
        panel,
        (thief, thief_queue),
    ));
}

/// Nor take the person to another workspace, bringing back a window there
/// or bringing it forward: showing the workspace moves the keys. Not even
/// from an empty desk, where nothing has them, as a launcher shown there
/// takes none.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_brings_back_no_window_on_another_workspace() {
    use perspicax_policy::{Action, Direction};

    let session = seated_on_two_workspaces("withheld-taskbar-workspace");
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    session.perform(Action::Workspace(Direction::Right));
    until(&mut other_queue, &mut other, |_| {
        heard(&other_ear).entered.is_none()
    });
    let (mut thief, mut thief_queue) = background_taskbar(&session);
    until(&mut thief_queue, &mut thief, |thief| {
        thief.task("other").is_some()
    });

    let handle = thief.task("other").expect("listed").handle.clone();
    handle.unset_minimized();
    thief_queue.roundtrip(&mut thief).expect("the request sent");
    assert!(
        titled(&session.facts.read(), "other").is_none(),
        "its workspace is not shown"
    );
    thief.activate("other");
    thief_queue.roundtrip(&mut thief).expect("the request sent");
    other_queue.roundtrip(&mut other).expect("dispatch");
    assert!(heard(&other_ear).entered.is_none(), "nothing has the keys");
    assert!(titled(&session.facts.read(), "other").is_none());

    session.stop(((other, other_queue), (thief, thief_queue)));
}

/// A taskbar the person's press reached brings a window forward with the
/// keys: a task clicked. Until they press anything else, and then it may
/// bring nothing forward.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_taskbar_the_persons_press_reached_brings_a_window_forward_until_they_press_again() {
    let session = seated("withheld-taskbar-pressed");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut panel, mut panel_queue) = panel(&session, 7);
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut panel_queue, &mut panel, |panel| {
        panel.task("other").is_some()
    });

    common::press_reaching(&session, &mut panel, &mut panel_queue);
    panel.activate("other");
    panel_queue.roundtrip(&mut panel).expect("the pick sent");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });

    types(&session, &mut other, &mut other_queue, "x");
    panel.activate("terminal");
    panel_queue.roundtrip(&mut panel).expect("the pick sent");
    other_queue.roundtrip(&mut other).expect("dispatch");
    queue.roundtrip(&mut desk).expect("dispatch");
    assert!(has_keys(&other, &other_ear, 0), "pressed since");
    assert!(!has_keys(&desk, &ear, 0));

    session.stop(((desk, queue), (other, other_queue), (panel, panel_queue)));
}

/// A taskbar of the program in use brings a window forward, a switcher
/// say: the person is behind what the program they use asks for.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_taskbar_of_the_program_in_use_brings_a_window_forward() {
    let session = seated("withheld-taskbar-in-use");
    let (mut desk, mut queue, _, _, _) = terminal(&session);
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    until(&mut queue, &mut desk, |desk| desk.task("other").is_some());

    desk.activate("other");
    queue.roundtrip(&mut desk).expect("the pick sent");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });

    session.stop(((desk, queue), (other, other_queue)));
}

/// A window that opened without the keys, picked while a launcher of the
/// taskbar's own has them -- the shell's pie, a task's menu -- comes
/// forward, and has them once the launcher goes: the person asked for it.
/// Not the window it came forward over, which the keys would go back to
/// were it still marked as wanting attention.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_picked_while_the_taskbars_own_launcher_has_the_keys_has_them_once_it_goes() {
    let session = seated("withheld-taskbar-launcher");
    let (mut desk, mut queue, _, _, ear) = terminal(&session);
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    let (mut panel, mut panel_queue) = panel(&session, 7);
    told_so(&mut panel, &mut panel_queue, "other", true);

    common::press_reaching(&session, &mut panel, &mut panel_queue);
    let panel_qh = panel_queue.handle();
    show_launcher(&mut panel, &mut panel_queue, &panel_qh);
    until(&mut queue, &mut desk, |desk| !has_keys(desk, &ear, 0));
    panel.activate("other");
    panel_queue.roundtrip(&mut panel).expect("the pick sent");
    session.wait_for(|facts| on_top(facts).as_deref() == Some("other"));

    panel.layers.clear();
    panel_queue
        .roundtrip(&mut panel)
        .expect("the launcher gone");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });
    told_so(&mut panel, &mut panel_queue, "other", false);

    session.stop(((desk, queue), (other, other_queue), (panel, panel_queue)));
}

/// The person clicks a panel, which never has the keys, and what its
/// taskbar asks next is theirs: the click reached it.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_taskbar_whose_panel_the_person_clicks_brings_a_window_forward() {
    use perspicax_policy::{Button, Mods};

    let session = seated("withheld-taskbar-clicked");
    let (desk, queue, _, _, _) = terminal(&session);
    let (mut other, mut other_queue, _, _, other_ear) = program(&session, "other");
    let (mut bar, mut bar_queue, bar_qh, bar_globals) = session.client();
    bar.bind_taskbar(&bar_globals, &bar_qh);
    bar.bind_pointer(&bar_globals, &bar_qh);
    bar.open_panel(&bar_qh, "panel", 40, 0xff22_2222);
    until(&mut bar_queue, &mut bar, |bar| {
        bar.layers_drawn > 0 && bar.task("other").is_some()
    });

    // On the panel, across the top of the screen.
    session.command(Do::Click {
        at: (640, 20),
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut bar_queue, &mut bar, |bar| bar.pressed.is_some());
    bar.activate("other");
    bar_queue.roundtrip(&mut bar).expect("the pick sent");
    until(&mut other_queue, &mut other, |other| {
        has_keys(other, &other_ear, 0)
    });

    session.stop(((desk, queue), (other, other_queue), (bar, bar_queue)));
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
    if steps == Steps::Launcher.name() {
        desk.open_launcher(&qh, STRIP, LAUNCHER);
    } else {
        desk.open_window(&qh, "probe", "probe");
    }
    loop {
        queue
            .blocking_dispatch(&mut desk)
            .expect("the compositor is there");
    }
}
