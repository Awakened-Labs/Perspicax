//! `[mouse]` bindings, as a person's hand sets them off and an agent's
//! never does, by a compositor with no screen.
//!
//! A button bound over the desktop takes a click there, and the same button
//! over a window is still the window's: a thumb button that flips
//! workspaces over the wallpaper is Back in a browser. A press a binding
//! takes is never seen by a client, and neither is its release. A
//! double-click binding waits for the second click. The wheel steps once a
//! notch, and bound all the way round it flips the other way from scroll
//! flipping, up and left forward. Per output, a binding acts on the monitor
//! under the pointer, not the focused window's. An agent's click or scroll
//! on the very same spot sets nothing off.
//!
//! The person's hand is [`Command::Click`] and [`Command::Scroll`], which
//! find what is under the pointer from the clients' buffers, so this needs
//! the `capture` feature. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Session, until};
use perspicax_compositor::{Backend, Command, Host, Virtual};
use perspicax_index::{Action as Verb, HostFacts, Layer, PointerButton, SurfaceKind};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::{
    Access, Action, Button, Context, Gesture, Grid, Mode, Mods, MouseBindings, MouseChord, Place,
    Shape, Side, Wheel,
};
use smithay_client_toolkit::shell::wlr_layer::Layer as Strip;
use wayland_client::{EventQueue, QueueHandle};

const SIDE: u32 = 0x113;
const EXTRA: u32 = 0x114;
const NEXT: Action = Action::CycleWorkspace { forward: true };

/// One monitor, 1280x800, and four workspaces in a row that does not wrap.
fn session(name: &str) -> Session {
    Session::start(
        name,
        Backend::Headless {
            outputs: vec![Virtual::numbered(1, (1280, 800))],
            workspaces: Shape {
                mode: Mode::Spanning,
                grid: Grid {
                    columns: 4,
                    rows: 1,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: false,
        },
    )
}

/// One window, drawn, with our pointer and a pager bound.
fn with_window(session: &Session) -> (Desk, EventQueue<Desk>, QueueHandle<Desk>) {
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    desk.open_window(&qh, "first", "first");
    until(&mut queue, &mut desk, |desk| {
        desk.drawn == 1 && desk.pager_done > 0
    });
    (desk, queue, qh)
}

/// `gesture`, with no modifiers, doing `action` in `context`.
fn bound(context: Context, gesture: Gesture, action: Action) -> MouseBindings {
    MouseBindings::default().bind(
        context,
        MouseChord {
            mods: Mods::default(),
            gesture,
        },
        action,
    )
}

/// A person's click at `at`, with no modifiers.
fn click(session: &Session, at: (i32, i32), button: Button) {
    session.command(Command::Click {
        at,
        button,
        mods: Mods::default(),
    });
}

/// A person's turn of the wheel at `at`, in 120ths of a notch, downward.
fn scroll(session: &Session, at: (i32, i32), down: i32) {
    session.command(Command::Scroll {
        at,
        v120: (0, down),
        mods: Mods::default(),
    });
}

/// The window's centre, once the facts have it.
fn centre_of_first(session: &Session) -> (i32, i32) {
    let facts = session.wait_for(|facts| geometry(facts, "first").is_some());
    let at = geometry(&facts, "first").expect("waited for");
    (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    )
}

/// Somewhere on the monitor the window is not.
fn beside_first(session: &Session) -> (i32, i32) {
    let facts = session.wait_for(|facts| geometry(facts, "first").is_some());
    let at = geometry(&facts, "first").expect("waited for");
    let spot = (1270, 790);
    assert!(
        !(at.x0..at.x1).contains(&f64::from(spot.0))
            || !(at.y0..at.y1).contains(&f64::from(spot.1)),
        "the window covers the corner: {at:?}"
    );
    spot
}

fn geometry(facts: &HostFacts, title: &str) -> Option<Rect> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
        .map(|surface| surface.geometry)
        .filter(|geometry| geometry.x1 > geometry.x0)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_desktop_binding_takes_a_click_on_the_desktop_and_leaves_one_on_a_window() {
    let session = session("mouse-desktop");
    let (mut desk, mut queue, _qh) = with_window(&session);
    session.command(Command::MouseBindings(bound(
        Context::Desktop,
        Gesture::Press(Button::Side),
        NEXT,
    )));

    // Over the window the thumb button is the window's, Back in a browser.
    click(&session, centre_of_first(&session), Button::Side);
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    assert_eq!(desk.buttons, [(SIDE, true), (SIDE, false)]);
    assert_eq!(desk.active_workspaces(), ["1"]);

    // Over the desktop it is the binding's.
    click(&session, beside_first(&session), Button::Side);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["2"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_press_a_binding_takes_reaches_no_client_and_nor_does_its_release() {
    let session = session("mouse-window");
    let (mut desk, mut queue, _qh) = with_window(&session);
    session.command(Command::MouseBindings(bound(
        Context::Window,
        Gesture::Press(Button::Side),
        NEXT,
    )));
    let centre = centre_of_first(&session);

    click(&session, centre, Button::Side);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["2"]
    });

    // Back to the window, and a button nothing binds after it: once that
    // has arrived, so would anything sent before it.
    session.perform(Action::GoToWorkspace(1));
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["1"]
    });
    click(&session, centre, Button::Extra);
    until(&mut queue, &mut desk, |desk| desk.buttons.len() >= 2);
    assert_eq!(desk.buttons, [(EXTRA, true), (EXTRA, false)]);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_double_click_binding_waits_for_the_second_click_and_a_third_begins_again() {
    let session = session("mouse-double");
    let (mut desk, mut queue, _qh) = with_window(&session);
    session.command(Command::MouseBindings(bound(
        Context::Desktop,
        Gesture::Double(Button::Side),
        NEXT,
    )));
    let spot = beside_first(&session);

    // Three clicks, sent together so they fall inside the double-click
    // time: a double-click, then a click on its own.
    for _ in 0..3 {
        click(&session, spot, Button::Side);
    }
    session.perform(NEXT);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["3"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_wheel_binding_steps_once_a_notch_and_leaves_the_wheel_to_a_window() {
    let session = session("mouse-wheel");
    let (mut desk, mut queue, _qh) = with_window(&session);
    session.command(Command::MouseBindings(bound(
        Context::Desktop,
        Gesture::Wheel(Wheel::Down),
        NEXT,
    )));

    scroll(&session, centre_of_first(&session), 120);
    until(&mut queue, &mut desk, |desk| desk.scrolls > 0);
    assert_eq!(desk.active_workspaces(), ["1"]);

    let spot = beside_first(&session);
    scroll(&session, spot, 240);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["3"]
    });
    // Half a notch is nothing yet; the second half makes one.
    scroll(&session, spot, 60);
    scroll(&session, spot, 60);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["4"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn wheel_bindings_turn_flipping_round_up_and_left_to_the_next_workspace() {
    let session = session("mouse-wheel-round");
    let (mut desk, mut queue, _qh) = with_window(&session);
    let previous = Action::CycleWorkspace { forward: false };
    let turned = [
        (Wheel::Up, NEXT),
        (Wheel::Left, NEXT),
        (Wheel::Down, previous.clone()),
        (Wheel::Right, previous),
    ]
    .into_iter()
    .fold(MouseBindings::default(), |bindings, (wheel, action)| {
        bindings.bind(
            Context::Desktop,
            MouseChord {
                mods: Mods::default(),
                gesture: Gesture::Wheel(wheel),
            },
            action,
        )
    });
    session.command(Command::MouseBindings(turned));
    let turn = |at, v120| {
        session.command(Command::Scroll {
            at,
            v120,
            mods: Mods::default(),
        });
    };

    // Over the window the wheel is the window's, whichever way it turns.
    turn(centre_of_first(&session), (0, -120));
    until(&mut queue, &mut desk, |desk| desk.scrolls > 0);
    assert_eq!(desk.active_workspaces(), ["1"]);

    let spot = beside_first(&session);
    for (v120, shown) in [
        ((0, -120), "2"),
        ((-120, 0), "3"),
        ((0, 120), "2"),
        ((120, 0), "1"),
    ] {
        turn(spot, v120);
        until(&mut queue, &mut desk, |desk| {
            desk.active_workspaces() == [shown]
        });
    }

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn per_output_a_binding_switches_the_monitor_under_the_pointer() {
    let session = Session::start(
        "mouse-per-output",
        Backend::Headless {
            outputs: vec![
                Virtual::numbered(1, (1280, 800)),
                Virtual {
                    place: Place::Beside {
                        side: Side::RightOf,
                        of: "HEADLESS-1".to_owned(),
                        offset: 0,
                    },
                    ..Virtual::numbered(2, (1280, 800))
                },
            ],
            workspaces: Shape {
                mode: Mode::PerOutput,
                grid: Grid {
                    columns: 4,
                    rows: 1,
                    wrap: false,
                },
            },
            access: Access::open(),
            person: false,
        },
    );
    let (mut desk, mut queue, _qh) = with_window(&session);
    session.wait_for(|facts| {
        facts.surfaces().iter().any(|surface| {
            surface.title.as_deref() == Some("first")
                && surface.focused_at.is_some()
                && surface.geometry.x1 <= 1280.0
        })
    });
    session.command(Command::MouseBindings(bound(
        Context::Desktop,
        Gesture::Press(Button::Side),
        NEXT,
    )));

    // A key would switch the focused window's monitor, the first. The hand
    // is on the second.
    click(&session, (1280 + 640, 400), Button::Side);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["1", "2"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_agents_click_and_scroll_never_set_off_a_binding() {
    let session = session("mouse-agent");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    desk.bind_pager(&globals, &qh);
    desk.open_strip(&qh, Strip::Background, "wallpaper", 200, 0xff33_6699);
    until(&mut queue, &mut desk, |desk| desk.pager_done > 0);
    let facts = session.wait_for(|facts| wallpaper(facts).is_some());
    let strip = wallpaper(&facts).expect("waited for");
    session.command(Command::MouseBindings(
        bound(Context::Desktop, Gesture::Press(Button::Left), NEXT).bind(
            Context::Desktop,
            MouseChord {
                mods: Mods::default(),
                gesture: Gesture::Wheel(Wheel::Down),
            },
            NEXT,
        ),
    ));

    let host = Host::new(&session.facts, &session.requests);
    let at = Rect::new(95.0, 95.0, 105.0, 105.0);
    host.act(
        strip,
        &Verb::Click {
            at,
            button: PointerButton::Left,
        },
    )
    .expect("dispatched");
    until(&mut queue, &mut desk, |desk| desk.buttons.len() == 2);
    host.act(
        strip,
        &Verb::Scroll {
            at,
            dx: 0.0,
            dy: 30.0,
        },
    )
    .expect("dispatched");
    until(&mut queue, &mut desk, |desk| desk.scrolls > 0);
    assert_eq!(
        desk.active_workspaces(),
        ["1"],
        "the agent's click and scroll went to the wallpaper, and set nothing off"
    );

    // The same spot, by a person's hand.
    click(&session, (100, 100), Button::Left);
    until(&mut queue, &mut desk, |desk| {
        desk.active_workspaces() == ["2"]
    });
    assert_eq!(
        desk.buttons.len(),
        2,
        "the person's click was the binding's"
    );

    session.stop((desk, queue));
}

/// Our wallpaper strip's id.
fn wallpaper(facts: &HostFacts) -> Option<SurfaceId> {
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
        .map(|surface| surface.id)
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_overlay_that_comes_up_under_a_still_pointer_has_it_and_gives_it_back_when_it_goes() {
    let session = session("mouse-repoint");
    let (mut desk, mut queue, _qh) = with_window(&session);
    // A person's click on the window leaves the pointer resting there.
    click(&session, centre_of_first(&session), Button::Left);
    until(&mut queue, &mut desk, |desk| {
        desk.entered.is_some() && desk.buttons.len() == 2
    });

    // A menu comes up over the whole screen, from another client, and the
    // hand does not move: the pointer is the menu's, and no longer the
    // window's, as if it had.
    let (mut menu, mut menu_queue, menu_qh, menu_globals) = session.client();
    menu.bind_pointer(&menu_globals, &menu_qh);
    menu.open_strip(&menu_qh, Strip::Overlay, "menu", 800, 0xff44_4444);
    until(&mut menu_queue, &mut menu, |menu| menu.entered.is_some());
    until(&mut queue, &mut desk, |desk| desk.entered.is_none());

    // It goes, and the window has the pointer back, still without a move.
    menu.layers.clear();
    menu_queue.roundtrip(&mut menu).expect("destroyed");
    until(&mut queue, &mut desk, |desk| desk.entered.is_some());

    session.stop(((desk, queue), (menu, menu_queue)));
}
