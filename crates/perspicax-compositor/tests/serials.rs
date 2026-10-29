//! The serial a client names, and the keys, by a compositor with no screen
//! (#187).
//!
//! A client asking for the keys names a serial, the input it is answering:
//! a click or a key given to it on the seat. An activation token carries
//! one, for a window to be brought forward with, as a link opened in a
//! browser already running is; so does a menu's grab, for the menu to take
//! the keys, as a right-click opens one. Nothing in either protocol stops a
//! client naming a serial it was never sent, or one it was sent long ago. So
//! a serial is believed only from the client that has the keys: what it
//! names is input the person gave it, or it is the client in use anyway. Any
//! other client's token is forgotten as it is made, and its menu dismissed.
//! Otherwise a window could come forward while the person typed elsewhere,
//! and take what they typed: a password at a terminal's `sudo` prompt, sent
//! on by the Enter that followed it.
//!
//! A kept token is good only until the person moves on (#257): until they
//! press a key or a button, or the keys go to another client -- a window
//! opening, the pointer under sloppy focus, Alt+Tab -- unless they leave a
//! launcher, as it goes and the program it started spends its token.
//! Otherwise the program they were using a moment ago could keep one, and
//! spend it once they had moved on to the terminal.
//!
//! Each test has programs, each a client of its own with a window, a
//! keyboard and a pointer. What each keyboard was told is what is asserted,
//! `enter` and `leave` alone, and which window the facts have on top. Only
//! the person's hand, a stand-in clicking or pointing, needs `capture`, to
//! find what is under it. Like the other live tests these bind a real
//! Wayland socket, so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use common::{Desk, Ear, Session, heard, menu_with_the_keyboard, until};
use perspicax_compositor::Backend;
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_node::SurfaceId;
use smithay_client_toolkit::shell::{WaylandSurface as _, xdg::XdgSurface as _};
use wayland_client::{EventQueue, Proxy as _, QueueHandle};

/// A serial no seat ever sent: what a client makes up.
const MADE_UP: u32 = 0x0bad_5eed;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)))
}

/// A session with a person at the seat, where alone a menu takes the keys.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 800)).with_person())
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
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

/// A program: a client of its own, with a keyboard, a pointer, a taskbar
/// and `xdg_activation_v1`, and a window called `title`, on screen and with
/// the keys, as a window that has just opened has them. With a person at
/// the seat, one that opens while another program's window has them opens
/// without them (#259), and the person picks it in the taskbar. Returns
/// what its keyboard hears, and its window to the facts.
fn program(
    session: &Session,
    title: &str,
) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>, Ear, SurfaceId) {
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.bind_pointer(&globals, &qh);
    desk.bind_taskbar(&globals, &qh);
    desk.bind_activation(&globals, &qh);
    desk.open_window(&qh, title, title);
    until(&mut queue, &mut desk, |desk| {
        desk.task(title).is_some()
            && titled(&session.facts.read(), title).is_some_and(|window| window.mapped)
    });
    let id = titled(&session.facts.read(), title).expect("waited for").id;
    if !has_keys(&desk, &ear) {
        // The person's press reaches the taskbar they pick it in (#270):
        // this program's own, bound as the shell for the stand-in to hand
        // the press to, and the only one bound so.
        desk.bind_shell(&globals, &qh);
        queue.roundtrip(&mut desk).expect("bound");
        common::press_reaching(session, &mut desk, &mut queue);
        desk.activate(title);
    }
    until(&mut queue, &mut desk, |desk| has_keys(desk, &ear));
    (desk, queue, qh, ear, id)
}

/// Whether this program's window has the keys, as its keyboard last heard.
fn has_keys(desk: &Desk, ear: &Ear) -> bool {
    heard(ear).entered.as_ref() == Some(&desk.windows[0].wl_surface().id())
}

/// Hear everything the compositor has sent so far: `asking` first, since
/// what it just asked for is what might have moved the keys, then `typing`,
/// so it hears any `leave` that came of it.
fn hear_all(
    asking: &mut Desk,
    asking_queue: &mut EventQueue<Desk>,
    typing: &mut Desk,
    typing_queue: &mut EventQueue<Desk>,
) {
    asking_queue.roundtrip(asking).expect("dispatch");
    typing_queue.roundtrip(typing).expect("dispatch");
}

/// The issue's own case. A program whose window opened first, and lost the
/// keys to the terminal the person types in, asks for its window to be
/// brought forward with a serial it was never sent. It comes forward with
/// nothing: the terminal keeps the keys, and stays on top.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_made_up_serial_brings_no_window_forward() {
    let session = session("serials-made-up");
    let (mut thief, mut thief_queue, thief_qh, thief_ear, _) = program(&session, "thief");
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut thief_queue, &mut thief, |thief| {
        !has_keys(thief, &thief_ear)
    });

    thief.ask_activation(&thief_qh, 0, MADE_UP);
    until(&mut thief_queue, &mut thief, |thief| {
        thief.tokens_spent == 1
    });
    hear_all(
        &mut thief,
        &mut thief_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(!has_keys(&thief, &thief_ear), "the thief has none of them");
    assert_eq!(
        on_top(&session.facts.read()).as_deref(),
        Some("terminal"),
        "the thief is not brought forward"
    );

    session.stop(((thief, thief_queue), (terminal, terminal_queue)));
}

/// A serial it really was sent, but while it had the keys, before the
/// person moved on to the terminal: the last one a program kept, to pull
/// its window forward with later. As made up as any other, now.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_serial_from_when_it_had_the_keys_brings_no_window_forward() {
    let session = session("serials-stale");
    let (mut thief, mut thief_queue, thief_qh, thief_ear, _) = program(&session, "thief");
    let kept = heard(&thief_ear).enter_serial.expect("entered");
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut thief_queue, &mut thief, |thief| {
        !has_keys(thief, &thief_ear)
    });

    thief.ask_activation(&thief_qh, 0, kept);
    until(&mut thief_queue, &mut thief, |thief| {
        thief.tokens_spent == 1
    });
    hear_all(
        &mut thief,
        &mut thief_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(!has_keys(&thief, &thief_ear), "the thief has none of them");
    assert_eq!(
        on_top(&session.facts.read()).as_deref(),
        Some("terminal"),
        "the thief is not brought forward"
    );

    session.stop(((thief, thief_queue), (terminal, terminal_queue)));
}

/// What the protocol is for: a link opened in the terminal the person types
/// in, in a browser already running behind it. The terminal, with the keys,
/// makes a token for the key that opened the link and hands it to the
/// browser, which spends it on its window. That comes forward, with the
/// keys. A launcher starting a program does the same.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_made_by_the_window_in_use_brings_another_programs_window_forward() {
    let session = session("serials-handed-on");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, terminal_qh, terminal_ear, _) =
        program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });

    let serial = heard(&terminal_ear).enter_serial.expect("entered");
    terminal.make_token(&terminal_qh, 0, serial);
    until(&mut terminal_queue, &mut terminal, |terminal| {
        !terminal.tokens.is_empty()
    });
    let token = terminal.tokens.remove(0);
    browser.spend_token(&token, 0);
    until(&mut browser_queue, &mut browser, |browser| {
        has_keys(browser, &browser_ear)
    });
    terminal_queue.roundtrip(&mut terminal).expect("dispatch");
    assert!(
        !has_keys(&terminal, &terminal_ear),
        "the terminal let the keys go"
    );
    session.wait_for(|facts| on_top(facts).as_deref() == Some("browser"));

    session.stop(((browser, browser_queue), (terminal, terminal_queue)));
}

/// A token made while the window's own menu has the keys, as one is when
/// "Open link" is picked from it, naming the input from before the menu
/// opened: the client in use is the one asking, whichever of its surfaces
/// holds the keys. The menu closes, as a picked item closes it, and the
/// browser the token is handed to comes forward with the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_made_while_its_own_menu_has_the_keys_is_kept() {
    let session = seated("serials-from-a-menu");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, terminal_qh, terminal_ear, page) =
        program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });
    let serial = heard(&terminal_ear).enter_serial.expect("entered");
    let menu = menu_with_the_keyboard(
        &session,
        &mut terminal,
        &mut terminal_queue,
        &terminal_qh,
        &terminal_ear,
        page,
    );

    terminal.make_token(&terminal_qh, 0, serial);
    until(&mut terminal_queue, &mut terminal, |terminal| {
        !terminal.tokens.is_empty()
    });
    drop(menu);
    until(&mut terminal_queue, &mut terminal, |terminal| {
        has_keys(terminal, &terminal_ear)
    });
    let token = terminal.tokens.remove(0);
    browser.spend_token(&token, 0);
    until(&mut browser_queue, &mut browser, |browser| {
        has_keys(browser, &browser_ear)
    });
    session.wait_for(|facts| on_top(facts).as_deref() == Some("browser"));

    session.stop(((browser, browser_queue), (terminal, terminal_queue)));
}

/// A token made once the window's own menu has closed, naming the input
/// given while it was open: "Open link" picked from it, the menu closing as
/// the toolkit acts on the item. Closing it gave the window the keys back,
/// with a serial newer than anything the menu was sent, and the token still
/// brings the browser it is handed to forward, with the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_naming_input_from_before_its_own_menu_closed_is_kept() {
    let session = seated("serials-after-a-menu");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, terminal_qh, terminal_ear, page) =
        program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });
    let menu = menu_with_the_keyboard(
        &session,
        &mut terminal,
        &mut terminal_queue,
        &terminal_qh,
        &terminal_ear,
        page,
    );
    // What the menu was sent as it took the keys: the serial it was opened
    // with, which a grab hands on.
    let picked = heard(&terminal_ear).enter_serial.expect("entered");
    drop(menu);
    until(&mut terminal_queue, &mut terminal, |terminal| {
        has_keys(terminal, &terminal_ear)
    });
    assert_ne!(
        heard(&terminal_ear).enter_serial,
        Some(picked),
        "the keys came back with a serial of their own"
    );

    terminal.make_token(&terminal_qh, 0, picked);
    until(&mut terminal_queue, &mut terminal, |terminal| {
        !terminal.tokens.is_empty()
    });
    let token = terminal.tokens.remove(0);
    browser.spend_token(&token, 0);
    until(&mut browser_queue, &mut browser, |browser| {
        has_keys(browser, &browser_ear)
    });
    session.wait_for(|facts| on_top(facts).as_deref() == Some("browser"));

    session.stop(((browser, browser_queue), (terminal, terminal_queue)));
}

/// A menu opened with a made-up serial by a program whose window lost the
/// keys to the terminal: nothing was clicked or pressed in it. It is
/// dismissed, as xdg-shell has a refused grab answered, and takes nothing:
/// had it taken the keys, its window would have kept them as it closed.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_menu_opened_by_a_program_without_the_keys_is_dismissed_and_takes_none() {
    let session = seated("serials-menu");
    let (mut thief, mut thief_queue, thief_qh, thief_ear, _) = program(&session, "thief");
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut thief_queue, &mut thief, |thief| {
        !has_keys(thief, &thief_ear)
    });

    let menu = thief.open_menu(
        &thief_qh,
        thief.windows[0].xdg_surface(),
        (40, 30),
        (120, 80),
        MADE_UP,
    );
    hear_all(
        &mut thief,
        &mut thief_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert_eq!(thief.popups_done, 1, "the menu is dismissed");
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert_ne!(
        heard(&thief_ear).entered.as_ref(),
        Some(&menu.wl_surface().id()),
        "the menu has none of them"
    );

    session.stop(((thief, thief_queue), (terminal, terminal_queue)));
}

/// Make a token from the program's window for the input `serial` names, as
/// a terminal opening a link does, and wait for it.
fn kept_token(
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    serial: u32,
) -> String {
    desk.make_token(qh, 0, serial);
    until(queue, desk, |desk| !desk.tokens.is_empty());
    desk.tokens.remove(0)
}

/// The issue's own case (#257), as it wrote it. The program the person was
/// using makes a token while it has the keys, as one making them all the
/// while would. The terminal opens, as a binding or a launcher opens one,
/// and takes the keys. The token, spent now, brings nothing forward: the
/// terminal keeps the keys, and stays on top.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_made_before_a_window_opened_with_the_keys_brings_none_forward() {
    let session = session("serials-opened-since");
    let (mut thief, mut thief_queue, thief_qh, thief_ear, _) = program(&session, "thief");
    let serial = heard(&thief_ear).enter_serial.expect("entered");
    let token = kept_token(&mut thief, &mut thief_queue, &thief_qh, serial);
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut thief_queue, &mut thief, |thief| {
        !has_keys(thief, &thief_ear)
    });

    thief.spend_token(&token, 0);
    hear_all(
        &mut thief,
        &mut thief_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(!has_keys(&thief, &thief_ear), "the thief has none of them");
    assert_eq!(
        on_top(&session.facts.read()).as_deref(),
        Some("terminal"),
        "the thief is not brought forward"
    );

    session.stop(((thief, thief_queue), (terminal, terminal_queue)));
}

/// A token the terminal made, handed to the browser, which spends it only
/// after the person has pressed something: a click in the terminal, to put
/// the cursor at its prompt. The keys never moved, but the person did
/// something since, and the browser stays where it is (#257).
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_made_before_the_persons_latest_press_brings_no_window_forward() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    let session = session("serials-pressed-since");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, terminal_qh, terminal_ear, _) =
        program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });
    let serial = heard(&terminal_ear).enter_serial.expect("entered");
    let token = kept_token(&mut terminal, &mut terminal_queue, &terminal_qh, serial);

    session.command(Command::Click {
        at: centre(&session.facts.read(), "terminal"),
        button: Button::Left,
        mods: Mods::default(),
    });
    // Heard, press and release, so the compositor has counted it.
    until(&mut terminal_queue, &mut terminal, |terminal| {
        terminal.buttons.len() == 2
    });

    browser.spend_token(&token, 0);
    hear_all(
        &mut browser,
        &mut browser_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(
        !has_keys(&browser, &browser_ear),
        "the browser has none of them"
    );
    assert_eq!(
        on_top(&session.facts.read()).as_deref(),
        Some("terminal"),
        "the browser is not brought forward"
    );

    session.stop(((browser, browser_queue), (terminal, terminal_queue)));
}

/// Under sloppy focus the keys follow the pointer, with nothing pressed:
/// the person points at the terminal, and it has them. A token the program
/// they left made while it had them brings nothing forward now (#257).
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_made_before_the_pointer_took_the_keys_brings_none_forward() {
    use perspicax_compositor::Command;
    use perspicax_policy::{Focus, FocusModel};

    let session = session("serials-pointed-since");
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    let (mut thief, mut thief_queue, thief_qh, thief_ear, _) = program(&session, "thief");
    until(&mut terminal_queue, &mut terminal, |terminal| {
        !has_keys(terminal, &terminal_ear)
    });
    let serial = heard(&thief_ear).enter_serial.expect("entered");
    let token = kept_token(&mut thief, &mut thief_queue, &thief_qh, serial);

    session.command(Command::FocusPolicy(Focus {
        model: FocusModel::Sloppy,
        autoraise: false,
    }));
    // The pointer starts at the origin.
    session.command(Command::Motion {
        by: uncovered(&session.facts.read(), "terminal", "thief"),
    });
    until(&mut terminal_queue, &mut terminal, |terminal| {
        has_keys(terminal, &terminal_ear)
    });

    thief.spend_token(&token, 0);
    hear_all(
        &mut thief,
        &mut thief_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(!has_keys(&thief, &thief_ear), "the thief has none of them");

    session.stop(((thief, thief_queue), (terminal, terminal_queue)));
}

/// A program keeps no more than eight tokens at once, so one making them in
/// a loop has nowhere to grow (#257). The terminal makes nine: the ninth is
/// forgotten as it is made, and spent brings nothing forward, while the
/// first still brings the browser forward.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_ninth_token_is_refused_while_its_program_keeps_eight() {
    let session = session("serials-eight-kept");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, terminal_qh, terminal_ear, _) =
        program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });
    let serial = heard(&terminal_ear).enter_serial.expect("entered");
    for _ in 0..9 {
        terminal.make_token(&terminal_qh, 0, serial);
    }
    until(&mut terminal_queue, &mut terminal, |terminal| {
        terminal.tokens.len() == 9
    });

    browser.spend_token(&terminal.tokens[8], 0);
    hear_all(
        &mut browser,
        &mut browser_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the ninth brings nothing forward"
    );

    browser.spend_token(&terminal.tokens[0], 0);
    until(&mut browser_queue, &mut browser, |browser| {
        has_keys(browser, &browser_ear)
    });
    session.wait_for(|facts| on_top(facts).as_deref() == Some("browser"));

    session.stop(((browser, browser_queue), (terminal, terminal_queue)));
}

/// A point at the middle of the window called `title`.
#[cfg(feature = "capture")]
fn centre(facts: &HostFacts, title: &str) -> (i32, i32) {
    let at = titled(facts, title).expect("open").geometry;
    (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    )
}

/// A point of the window called `under` that `over` does not cover: its top
/// left, since `over` opened after it, down and right of it.
#[cfg(feature = "capture")]
fn uncovered(facts: &HostFacts, under: &str, over: &str) -> (i32, i32) {
    let under = titled(facts, under).expect("open").geometry;
    let over = titled(facts, over).expect("open").geometry;
    assert!(
        over.x0 > under.x0 + 10.0 && over.y0 > under.y0 + 10.0,
        "cascaded: {under:?} under {over:?}"
    );
    ((under.x0 + 5.0) as i32, (under.y0 + 5.0) as i32)
}

/// The person goes to an empty workspace, to start something there, and
/// nothing has the keys. A token the program they left made while it had
/// them brings nothing back (#257): spent, it would have shown that
/// workspace again, and its window with the keys.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_token_made_before_the_person_left_for_an_empty_workspace_brings_nothing_back() {
    use perspicax_compositor::Virtual;
    use perspicax_policy::{Access, Action, Direction, Grid, Mode, Shape};

    let session = Session::start(
        "serials-left-for-empty",
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
            person: false,
            opacity: Default::default(),
            agent: Default::default(),
        },
    );
    let (mut thief, mut thief_queue, thief_qh, thief_ear, _) = program(&session, "thief");
    let serial = heard(&thief_ear).enter_serial.expect("entered");
    let token = kept_token(&mut thief, &mut thief_queue, &thief_qh, serial);

    session.perform(Action::Workspace(Direction::Right));
    until(&mut thief_queue, &mut thief, |thief| {
        !has_keys(thief, &thief_ear)
    });

    thief.spend_token(&token, 0);
    thief_queue.roundtrip(&mut thief).expect("dispatch");
    assert!(
        !has_keys(&thief, &thief_ear),
        "the thief has none of the keys"
    );
    assert!(
        titled(&session.facts.read(), "thief").is_some_and(|thief| !thief.mapped),
        "its workspace is not shown again"
    );

    session.stop((thief, thief_queue));
}

/// How tall the notifier's strip is, across the top of the screen.
#[cfg(feature = "capture")]
const NOTICE: u32 = 60;

/// A point on the notifier's strip.
#[cfg(feature = "capture")]
const ON_NOTICE: (i32, i32) = (640, 30);

/// A notification daemon, as mako is: a client of its own with a pointer
/// and `xdg_activation_v1`, and a strip on `top` that never asks for a key,
/// drawn. A notification's action is clicked on it, and it makes a token for
/// that click, for the program whose notification it was.
#[cfg(feature = "capture")]
fn notifier(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    use smithay_client_toolkit::shell::wlr_layer::Layer;

    let (mut notifier, mut queue, qh, globals) = session.client();
    notifier.bind_pointer(&globals, &qh);
    notifier.bind_activation(&globals, &qh);
    notifier.open_strip(&qh, Layer::Top, "notifications", NOTICE, 0xff22_5588);
    until(&mut queue, &mut notifier, |notifier| {
        notifier.layers_drawn == 1
    });
    (notifier, queue, qh)
}

/// The person clicks the notifier's strip; returns the serial of the press
/// it was sent.
#[cfg(feature = "capture")]
fn click_notice(session: &Session, notifier: &mut Desk, queue: &mut EventQueue<Desk>) -> u32 {
    use perspicax_compositor::Command;
    use perspicax_policy::{Button, Mods};

    session.command(Command::Click {
        at: ON_NOTICE,
        button: Button::Left,
        mods: Mods::default(),
    });
    until(queue, notifier, |notifier| notifier.buttons.len() == 2);
    notifier.pressed.expect("pressed")
}

/// The notifier makes a token from its strip, naming `serial`, and waits for
/// it.
#[cfg(feature = "capture")]
fn notice_token(
    notifier: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    serial: u32,
) -> String {
    let strip = notifier.layers[0].0.wl_surface().clone();
    notifier.make_token_from(qh, &strip, serial);
    until(queue, notifier, |notifier| !notifier.tokens.is_empty());
    notifier.tokens.remove(0)
}

/// A notification's action clicked (#257). The notifier never has the keys,
/// so the click it was just sent is what vouches for the token it makes, and
/// the browser it is handed to comes forward with the keys, taking them
/// from the terminal the person was typing in.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_the_person_just_clicked_brings_a_window_forward() {
    let session = session("serials-notified");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });
    let (mut notifier, mut notifier_queue, notifier_qh) = notifier(&session);

    let clicked = click_notice(&session, &mut notifier, &mut notifier_queue);
    let token = notice_token(&mut notifier, &mut notifier_queue, &notifier_qh, clicked);
    browser.spend_token(&token, 0);
    until(&mut browser_queue, &mut browser, |browser| {
        has_keys(browser, &browser_ear)
    });
    terminal_queue.roundtrip(&mut terminal).expect("dispatch");
    assert!(
        !has_keys(&terminal, &terminal_ear),
        "the terminal let the keys go"
    );
    session.wait_for(|facts| on_top(facts).as_deref() == Some("browser"));

    session.stop((
        (browser, browser_queue),
        (terminal, terminal_queue),
        (notifier, notifier_queue),
    ));
}

/// The person clicked the notifier, then moved on: a window opened and took
/// the keys. A token the notifier makes now, naming that click, is
/// forgotten as it is made, and brings nothing forward (#257).
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_naming_a_click_the_person_has_moved_on_from_brings_nothing_forward() {
    let session = session("serials-notified-moved-on");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut notifier, mut notifier_queue, notifier_qh) = notifier(&session);
    let clicked = click_notice(&session, &mut notifier, &mut notifier_queue);
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });

    let token = notice_token(&mut notifier, &mut notifier_queue, &notifier_qh, clicked);
    browser.spend_token(&token, 0);
    hear_all(
        &mut browser,
        &mut browser_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(
        !has_keys(&browser, &browser_ear),
        "the browser has none of them"
    );

    session.stop((
        (browser, browser_queue),
        (terminal, terminal_queue),
        (notifier, notifier_queue),
    ));
}

/// The person clicked the notifier, but the token it makes names a serial
/// it was never sent. Only the press it was sent vouches, and the browser
/// stays where it is (#257).
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_layer_naming_a_press_it_was_not_sent_brings_nothing_forward() {
    let session = session("serials-notified-made-up");
    let (mut browser, mut browser_queue, _, browser_ear, _) = program(&session, "browser");
    let (mut terminal, mut terminal_queue, _, terminal_ear, _) = program(&session, "terminal");
    until(&mut browser_queue, &mut browser, |browser| {
        !has_keys(browser, &browser_ear)
    });
    let (mut notifier, mut notifier_queue, notifier_qh) = notifier(&session);

    click_notice(&session, &mut notifier, &mut notifier_queue);
    let token = notice_token(&mut notifier, &mut notifier_queue, &notifier_qh, MADE_UP);
    browser.spend_token(&token, 0);
    hear_all(
        &mut browser,
        &mut browser_queue,
        &mut terminal,
        &mut terminal_queue,
    );
    assert!(
        has_keys(&terminal, &terminal_ear),
        "the terminal keeps the keys"
    );
    assert!(
        !has_keys(&browser, &browser_ear),
        "the browser has none of them"
    );

    session.stop((
        (browser, browser_queue),
        (terminal, terminal_queue),
        (notifier, notifier_queue),
    ));
}
