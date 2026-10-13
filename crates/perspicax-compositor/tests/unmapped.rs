//! Windows their clients unmap, by attaching no buffer to them.
//!
//! xdg-shell says a toplevel with a null buffer attached is unmapped: it
//! cannot be shown, and it returns to the state it had right after
//! `get_toplevel`. Its client still holds it, and may map it again. Issue
//! #102: perspicax kept such a window in the space as a frame around nothing,
//! in the taskbars and in the keyboard's way. These tests unmap windows on
//! screen, behind a tab, minimized and with a menu open, and check that each
//! goes as a closed one does, and that its client is told nothing while it
//! is away.
//!
//! Like the other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Configured, Desk, Said, Session, WINDOW, configured, heard, until};
use perspicax_compositor::Backend;
#[cfg(feature = "capture")]
use perspicax_compositor::Command;
use perspicax_index::{HostFacts, SurfaceFacts};
use perspicax_node::{Rect, SurfaceId};
use perspicax_policy::Action;
#[cfg(feature = "capture")]
use perspicax_policy::{Button, Mods};
use smithay_client_toolkit::shell::{WaylandSurface as _, xdg::XdgSurface as _};
use wayland_client::{EventQueue, Proxy as _, QueueHandle};
#[cfg(feature = "capture")]
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::State;

const MENU: u32 = 0xff33_6699;

fn session(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)))
}

/// A session with a person at the seat, where the keyboard moves on from a
/// window that goes, and a menu takes the keyboard.
fn seated(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).with_person())
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// Open a window called `title`, wait until it has drawn and the facts say
/// it is on screen, and return its id.
fn opened(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    title: &str,
) -> SurfaceId {
    desk.open_window(qh, title, title);
    until(queue, desk, |_| {
        titled(&session.facts.read(), title).is_some_and(|window| window.mapped)
    });
    titled(&session.facts.read(), title).expect("waited for").id
}

/// Unmap the `nth` window as xdg-shell has a client do it: attach no buffer,
/// and commit. Sent, and taken in, before this returns.
fn unmap(desk: &mut Desk, queue: &mut EventQueue<Desk>, nth: usize) {
    let surface = desk.windows[nth].wl_surface();
    surface.attach(None, 0, 0);
    surface.commit();
    queue.roundtrip(desk).expect("the commit taken in");
}

/// How many configures the `nth` window has been sent.
fn configures(desk: &Desk, nth: usize) -> usize {
    desk.said_to(desk.windows[nth].wl_surface())
        .iter()
        .filter(|said| matches!(said, Said::Configure(_)))
        .count()
}

/// #102: the window leaves the facts, the wlr taskbar and the ext list, as
/// a closed one does. Its client still holds it, and is told nothing while
/// it is away: a configure would be the answer to a request to map again,
/// and this client made none.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_its_client_unmaps_leaves_the_facts_and_the_taskbars() {
    let session = session("unmap-leaves");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.bind_list(&globals, &qh);
    let gone = opened(&session, &mut desk, &mut queue, &qh, "gone");
    until(&mut queue, &mut desk, |desk| {
        desk.task_titles() == ["gone"] && desk.listed_titles() == ["gone"]
    });
    let told = configures(&desk, 0);

    unmap(&mut desk, &mut queue, 0);
    session.wait_for(|facts| facts.surface(gone).is_none() && facts.surfaces().is_empty());
    until(&mut queue, &mut desk, |desk| {
        desk.task_titles().is_empty() && desk.listed_titles().is_empty()
    });
    assert!(
        desk.tasked.iter().all(|task| task.closed),
        "its taskbar handle closed"
    );
    assert_eq!(configures(&desk, 0), told, "told something while unmapped");

    session.stop((desk, queue));
}

/// #102: with a person at the seat, the keyboard moves on from a window its
/// client unmaps to the window now on top, which the taskbar shows in use,
/// as when a window closes.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_its_client_unmaps_gives_the_keyboard_to_the_window_below() {
    let session = seated("unmap-keyboard");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    opened(&session, &mut desk, &mut queue, &qh, "below");
    let gone = opened(&session, &mut desk, &mut queue, &qh, "gone");
    let (below, unmapped) = (
        desk.windows[0].wl_surface().id(),
        desk.windows[1].wl_surface().id(),
    );
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&unmapped)
    });
    let told = configures(&desk, 1);

    unmap(&mut desk, &mut queue, 1);
    until(&mut queue, &mut desk, |desk| {
        heard(&ear).entered.as_ref() == Some(&below)
            && desk
                .task("below")
                .is_some_and(|task| task.is(State::Activated))
    });
    session.wait_for(|facts| facts.surface(gone).is_none());
    assert_eq!(
        desk.activations_of(0).last(),
        Some(&true),
        "the window below was told it is in use"
    );
    assert_eq!(configures(&desk, 1), told, "told something while unmapped");

    session.stop((desk, queue));
}

/// #102: a window unmapped with its own menu open, holding the keyboard. The
/// menu is dismissed, as a click outside it would dismiss it, and the
/// keyboard goes to the window now on top rather than staying with the menu
/// of a window nobody can see.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_unmapped_with_its_menu_open_dismisses_the_menu_and_the_keyboard_moves_on() {
    let session = seated("unmap-menu");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    opened(&session, &mut desk, &mut queue, &qh, "below");
    let page = opened(&session, &mut desk, &mut queue, &qh, "page");
    let (below, window) = (
        desk.windows[0].wl_surface().id(),
        desk.windows[1].wl_surface().id(),
    );
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });
    let serial = heard(&ear).enter_serial.expect("entered");
    let menu = desk.open_menu(
        &qh,
        desk.windows[1].xdg_surface(),
        (40, 30),
        (120, 80),
        serial,
    );
    let grabbed = menu.wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&grabbed)
    });
    configured(&mut desk, &mut queue, 1);
    desk.paint(menu.wl_surface(), (120, 80), MENU);
    menu.wl_surface().commit();

    unmap(&mut desk, &mut queue, 1);
    until(&mut queue, &mut desk, |desk| {
        desk.popups_done == 1 && heard(&ear).entered.as_ref() == Some(&below)
    });
    session.wait_for(|facts| facts.surface(page).is_none());
    until(&mut queue, &mut desk, |desk| {
        desk.task("below")
            .is_some_and(|task| task.is(State::Activated))
    });

    session.stop((desk, queue));
}

/// #102: a menu or tooltip of a window its client unmaps is dismissed with
/// it, so it does not come back with the window; and one opened from the
/// window while it is unmapped is dismissed at once, since a popup needs its
/// window on screen.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_popups_of_a_window_its_client_unmaps_are_dismissed() {
    let session = session("unmap-popups");
    let (mut desk, mut queue, qh, _) = session.client();
    opened(&session, &mut desk, &mut queue, &qh, "page");
    let tooltip = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    configured(&mut desk, &mut queue, 1);
    desk.paint(tooltip.wl_surface(), (120, 80), MENU);
    tooltip.wl_surface().commit();
    queue.roundtrip(&mut desk).expect("the tooltip drawn");

    unmap(&mut desk, &mut queue, 0);
    until(&mut queue, &mut desk, |desk| desk.popups_done == 1);

    let late = desk.open_popup(&qh, desk.windows[0].xdg_surface(), (40, 30), (120, 80));
    late.wl_surface().commit();
    until(&mut queue, &mut desk, |desk| desk.popups_done == 2);
    assert_eq!(desk.popups_configured, 1, "the late popup was configured");

    session.stop((desk, queue));
}

/// #102: a minimized window its client unmaps leaves the taskbar too: it was
/// off the screen, but still the person's to restore.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_minimized_window_its_client_unmaps_leaves_the_taskbar() {
    let session = session("unmap-minimized");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let gone = opened(&session, &mut desk, &mut queue, &qh, "gone");
    session.perform(Action::Minimize);
    session.wait_for(|facts| facts.surface(gone).is_some_and(|window| !window.mapped));
    until(&mut queue, &mut desk, |desk| desk.task_titles() == ["gone"]);

    unmap(&mut desk, &mut queue, 0);
    session.wait_for(|facts| facts.surface(gone).is_none());
    until(&mut queue, &mut desk, |desk| desk.task_titles().is_empty());

    session.stop((desk, queue));
}

/// #102: the tab in front of a group, unmapped by its client, gives its
/// place to the tab behind it, as it does when it closes: where it was, and
/// the size it had. That size is taken before the window is measured with
/// nothing in it, which a window that declares no geometry, measured only
/// from what it draws, would otherwise have as none.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_front_tab_its_client_unmaps_brings_the_tab_behind_it_forward_in_its_place() {
    let session = session("unmap-tab");
    let (mut desk, mut queue, qh, _) = session.client();
    let behind = opened(&session, &mut desk, &mut queue, &qh, "behind");
    desk.open_declaring(&qh, "front", None);
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), "front").is_some_and(|window| window.mapped)
    });
    let front = titled(&session.facts.read(), "front")
        .expect("waited for")
        .id;
    session.perform(Action::TabWithPrevious);
    let grouped = session.wait_for(|facts| {
        facts.surface(behind).is_some_and(|window| !window.mapped)
            && facts.surface(front).is_some_and(|window| window.mapped)
    });
    let place = grouped.surface(front).expect("waited for").geometry;
    let told = configures(&desk, 0);

    unmap(&mut desk, &mut queue, 1);
    let after = session.wait_for(|facts| {
        facts.surface(front).is_none() && facts.surface(behind).is_some_and(|window| window.mapped)
    });
    let heir = after.surface(behind).expect("waited for");
    assert_eq!(heir.geometry, place, "the tab behind took its place");
    assert_eq!(heir.behind_tab, None);
    until(&mut queue, &mut desk, |desk| configures(desk, 0) > told);
    let sizes: Vec<_> = desk
        .said_to(desk.windows[0].wl_surface())
        .into_iter()
        .filter_map(|said| match said {
            Said::Configure(configured) => Some(configured.size),
            _ => None,
        })
        .collect();
    assert_eq!(
        sizes.last(),
        Some(&Some(WINDOW)),
        "the tab behind was told the size the unmapped tab had: {sizes:?}"
    );

    session.stop((desk, queue));
}

/// #102: a null buffer on a window that never drew unmaps nothing: it was
/// never mapped. Smithay counts it so too.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_null_buffer_on_a_window_that_never_drew_changes_nothing() {
    let session = session("unmap-never-drew");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_blank(&qh, "blank");
    until(&mut queue, &mut desk, |desk| {
        desk.blank_configured >= 1 && desk.task_titles() == ["blank"]
    });
    let facts = session.wait_for(|facts| titled(facts, "blank").is_some());
    let id = titled(&facts, "blank").expect("waited for").id;

    let surface = desk.blank[0].wl_surface();
    surface.attach(None, 0, 0);
    surface.commit();
    queue.roundtrip(&mut desk).expect("the commit taken in");
    queue.roundtrip(&mut desk).expect("the taskbar told");
    assert!(
        session.facts.read().surface(id).is_some(),
        "the window left the facts"
    );
    assert_eq!(desk.task_titles(), ["blank"], "the window left the taskbar");

    session.stop((desk, queue));
}

/// #102: a client may destroy a window it has unmapped. Nothing is left of
/// it, and the compositor carries on.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_destroyed_after_its_client_unmapped_it_is_forgotten_quietly() {
    let session = session("unmap-then-destroy");
    let (mut desk, mut queue, qh, _) = session.client();
    let gone = opened(&session, &mut desk, &mut queue, &qh, "gone");
    unmap(&mut desk, &mut queue, 0);
    session.wait_for(|facts| facts.surface(gone).is_none());

    desk.windows.clear();
    queue.roundtrip(&mut desk).expect("destroyed");
    let next = opened(&session, &mut desk, &mut queue, &qh, "next");
    let facts = session.facts.read();
    assert_eq!(facts.surfaces().len(), 1, "only the new window");
    assert!(facts.surface(next).is_some());

    session.stop((desk, queue));
}

/// The configures the `nth` window has been sent, as it read them.
fn configured_as(desk: &Desk, nth: usize) -> Vec<Configured> {
    desk.said_to(desk.windows[nth].wl_surface())
        .into_iter()
        .filter_map(|said| match said {
            Said::Configure(configured) => Some(configured),
            _ => None,
        })
        .collect()
}

/// Unmap the `nth` window, and wait until the window with id `gone` has
/// left the facts. How many configures it had been sent by then.
fn unmapped(
    session: &Session,
    desk: &mut Desk,
    queue: &mut EventQueue<Desk>,
    nth: usize,
    gone: SurfaceId,
) -> usize {
    unmap(desk, queue, nth);
    session.wait_for(|facts| facts.surface(gone).is_none());
    configures(desk, nth)
}

/// #102: a client asks for a window it unmapped as it asks for a new one:
/// it commits with nothing in it, and waits for a configure. None ever
/// came: perspicax configured a toplevel only at `get_toplevel`. It is
/// answered as a new window is, active and with nothing else, its last
/// state forgotten. Drawn, it is back as a new window: an id nothing has
/// seen, and a new handle in the taskbar.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_asked_for_again_is_configured_and_back_as_new_when_it_draws() {
    let session = session("unmap-again");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let gone = opened(&session, &mut desk, &mut queue, &qh, "again");
    until(&mut queue, &mut desk, |desk| {
        desk.task_titles() == ["again"]
    });
    let handle = desk.task("again").expect("announced").handle.clone();
    let told = unmapped(&session, &mut desk, &mut queue, 0, gone);

    desk.windows[0].commit();
    until(&mut queue, &mut desk, |desk| configures(desk, 0) > told);
    let answer = configured_as(&desk, 0)[told];
    assert!(
        answer.activated
            && !answer.maximized
            && !answer.fullscreen
            && !answer.tiled
            && answer.size.is_none(),
        "not answered as a new window: {answer:?}"
    );

    // The client drew its answer: back.
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), "again").is_some_and(|window| window.mapped)
    });
    let back = titled(&session.facts.read(), "again")
        .expect("waited for")
        .id;
    assert_ne!(back, gone, "back with the id it had");
    until(&mut queue, &mut desk, |desk| {
        desk.task_titles() == ["again"]
    });
    assert_ne!(
        desk.task("again").expect("announced").handle,
        handle,
        "announced on the handle that was closed"
    );

    session.stop((desk, queue));
}

/// #102: with a person at the seat, a window mapped again takes the
/// keyboard, as a new window does, and the window that had it is told it no
/// longer is in use.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_mapped_again_takes_the_keyboard_as_a_new_window_does() {
    let session = seated("unmap-again-keyboard");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    let ear = desk.bind_keyboard(&globals, &qh);
    opened(&session, &mut desk, &mut queue, &qh, "below");
    let gone = opened(&session, &mut desk, &mut queue, &qh, "again");
    let (below, again) = (
        desk.windows[0].wl_surface().id(),
        desk.windows[1].wl_surface().id(),
    );
    unmapped(&session, &mut desk, &mut queue, 1, gone);
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&below)
    });

    desk.windows[1].commit();
    until(&mut queue, &mut desk, |desk| {
        heard(&ear).entered.as_ref() == Some(&again)
            && desk
                .task("again")
                .is_some_and(|task| task.is(State::Activated))
    });
    assert_eq!(desk.activations_of(1).last(), Some(&true));
    assert_eq!(
        desk.activations_of(0).last(),
        Some(&false),
        "the window below was not told it is no longer in use"
    );

    session.stop((desk, queue));
}

/// #102: the window is back from its first picture and not before. A
/// client may commit with nothing in it for reasons of its own, as this one
/// does, answering its configure with nothing drawn: on screen then, the
/// window would again be a frame around nothing, in the taskbar.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_asked_for_again_is_not_back_until_it_draws() {
    const BLANK: u32 = 0xff20_2020;
    let session = session("unmap-again-blank");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_blank(&qh, "blank");
    until(&mut queue, &mut desk, |desk| desk.blank_configured >= 1);
    let surface = desk.blank[0].wl_surface().clone();
    desk.paint(&surface, WINDOW, BLANK);
    surface.commit();
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), "blank").is_some_and(|window| window.mapped)
    });
    let gone = titled(&session.facts.read(), "blank")
        .expect("waited for")
        .id;
    surface.attach(None, 0, 0);
    surface.commit();
    queue.roundtrip(&mut desk).expect("the commit taken in");
    session.wait_for(|facts| facts.surface(gone).is_none());
    let answered = desk.blank_configured;

    desk.blank[0].commit();
    until(&mut queue, &mut desk, |desk| {
        desk.blank_configured > answered
    });
    queue
        .roundtrip(&mut desk)
        .expect("the empty answer taken in");
    queue.roundtrip(&mut desk).expect("the taskbar told");
    assert!(
        titled(&session.facts.read(), "blank").is_none(),
        "back before it drew"
    );
    assert!(
        desk.task_titles().is_empty(),
        "in the taskbar before it drew"
    );

    desk.paint(&surface, WINDOW, BLANK);
    surface.commit();
    until(&mut queue, &mut desk, |desk| {
        titled(&session.facts.read(), "blank").is_some_and(|window| window.mapped)
            && desk.task_titles() == ["blank"]
    });

    session.stop((desk, queue));
}

/// #102: a window maximized when its client unmapped it comes back as a new
/// window does, at the size it draws: whatever perspicax told it before is
/// discarded with the unmapping.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_maximized_when_its_client_unmapped_it_comes_back_unmaximized() {
    let session = seated("unmap-maximized");
    let (mut desk, mut queue, qh, _) = session.client();
    let gone = opened(&session, &mut desk, &mut queue, &qh, "max");
    session.perform(Action::ToggleMaximize);
    until(&mut queue, &mut desk, |desk| {
        configured_as(desk, 0)
            .last()
            .is_some_and(|configured| configured.maximized)
    });
    let told = unmapped(&session, &mut desk, &mut queue, 0, gone);

    desk.windows[0].commit();
    until(&mut queue, &mut desk, |desk| configures(desk, 0) > told);
    let answer = configured_as(&desk, 0)[told];
    assert!(
        !answer.maximized && answer.size.is_none(),
        "told to be maximized again: {answer:?}"
    );
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), "max").is_some_and(|window| window.mapped)
    });
    let facts = session.facts.read();
    let back = titled(&facts, "max").expect("waited for");
    assert_eq!(
        back.geometry.size(),
        Rect::new(0.0, 0.0, f64::from(WINDOW.0), f64::from(WINDOW.1)).size(),
        "not at the size it drew"
    );

    session.stop((desk, queue));
}

/// #102: a client may ask for its window to be maximized before it maps
/// it, and a window it unmapped is no different. Asked while the window is
/// away, it is maximized once it is back, with a person at the seat; the
/// configure that maps it again is what answers the asking.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_asked_to_be_maximized_while_unmapped_is_maximized_once_back() {
    let session = seated("unmap-asks-maximized");
    let (mut desk, mut queue, qh, _) = session.client();
    let gone = opened(&session, &mut desk, &mut queue, &qh, "max");
    let told = unmapped(&session, &mut desk, &mut queue, 0, gone);

    desk.windows[0].set_maximized();
    queue.roundtrip(&mut desk).expect("asked");
    assert_eq!(
        configures(&desk, 0),
        told,
        "answered before it was asked for"
    );
    desk.windows[0].commit();
    until(&mut queue, &mut desk, |desk| {
        configured_as(desk, 0)[told..]
            .iter()
            .any(|configured| configured.maximized && configured.size.is_some())
    });
    assert!(
        !configured_as(&desk, 0)[told].maximized,
        "maximized before it was back"
    );

    session.stop((desk, queue));
}

/// #102: a client that draws in a window it unmapped without asking for it
/// first is answered all the same: the window is back, and configured.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_drawn_again_without_asking_is_back_and_configured() {
    let session = session("unmap-draws-again");
    let (mut desk, mut queue, qh, _) = session.client();
    let gone = opened(&session, &mut desk, &mut queue, &qh, "eager");
    let told = unmapped(&session, &mut desk, &mut queue, 0, gone);

    let surface = desk.windows[0].wl_surface().clone();
    desk.paint(&surface, WINDOW, 0xffff_ffff);
    surface.commit();
    until(&mut queue, &mut desk, |desk| {
        configures(desk, 0) > told
            && titled(&session.facts.read(), "eager")
                .is_some_and(|window| window.mapped && window.id != gone)
    });

    session.stop((desk, queue));
}

/// #102: a window that asked to be told when it may draw, as GTK does with
/// every picture, and was unmapped before it was told. It is told all the
/// same, so the configure that answers its asking again is drawn, and the
/// window comes back: perspicax told only the windows in the space.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_waiting_to_draw_when_unmapped_is_told_it_may_and_comes_back() {
    let session = session("unmap-patient");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_patient(&qh, "patient", "patient");
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), "patient").is_some_and(|window| window.mapped)
    });
    let gone = titled(&session.facts.read(), "patient")
        .expect("waited for")
        .id;
    let surface = desk.windows[0].wl_surface().clone();
    surface.attach(None, 0, 0);
    desk.ask_frame(&qh, &surface);
    queue.roundtrip(&mut desk).expect("the commit taken in");
    session.wait_for(|facts| facts.surface(gone).is_none());

    desk.windows[0].commit();
    until(&mut queue, &mut desk, |_| {
        titled(&session.facts.read(), "patient").is_some_and(|window| window.mapped)
    });

    session.stop((desk, queue));
}

/// #102: a window whose frame perspicax draws comes back with its titlebar
/// on screen. Placed at the corner of a monitor too small to cascade on, a
/// new window has it fitted when it says who draws its frame. One mapped
/// again said so long before, and is fitted as it is mapped.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_framed_window_mapped_again_has_its_titlebar_on_screen() {
    let session = Session::start("unmap-framed", Backend::headless((100, 100)));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_framed(&qh, "framed");
    let framed = |facts: &HostFacts| {
        titled(facts, "framed")
            .filter(|window| window.mapped && !window.frame.is_empty())
            .map(|window| (window.id, window.frame.clone()))
    };
    until(&mut queue, &mut desk, |_| {
        framed(&session.facts.read()).is_some()
    });
    let (gone, frame) = framed(&session.facts.read()).expect("waited for");
    assert!(frame.iter().all(|strip| strip.y0 >= 0.0), "{frame:?}");
    unmapped(&session, &mut desk, &mut queue, 0, gone);

    desk.windows[0].commit();
    until(&mut queue, &mut desk, |_| {
        framed(&session.facts.read()).is_some_and(|(id, _)| id != gone)
    });
    let (_, frame) = framed(&session.facts.read()).expect("waited for");
    assert!(
        frame.iter().all(|strip| strip.y0 >= 0.0),
        "its titlebar above the screen: {frame:?}"
    );

    session.stop((desk, queue));
}

/// #102: a window unmapped by its client in the middle of a resize is told
/// nothing more of it: not as the pointer goes on moving, nor as the button
/// comes up. Every operation on an unmapped window is cancelled, and a
/// configure now is what its client waits for to map it again: the resize
/// would have handed it one, with a size, before it asked.
#[cfg(feature = "capture")]
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_resize_under_way_tells_a_window_its_client_unmapped_nothing_more() {
    let session = seated("unmap-resize");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_pointer(&globals, &qh);
    opened(&session, &mut desk, &mut queue, &qh, "below");
    let page = opened(&session, &mut desk, &mut queue, &qh, "page");
    let at = session
        .facts
        .read()
        .surface(page)
        .expect("waited for")
        .geometry;
    let middle = (
        ((at.x0 + at.x1) / 2.0).round() as i32,
        ((at.y0 + at.y1) / 2.0).round() as i32,
    );
    // The person presses on the page, which asks to be resized from its
    // corner, as a toolkit drawing its own frame does, and drags.
    session.command(Command::Press {
        at: middle,
        button: Button::Left,
        mods: Mods::default(),
    });
    until(&mut queue, &mut desk, |desk| desk.pressed.is_some());
    let seat = desk.seat.clone().expect("our pointer's seat");
    desk.windows[1].resize(
        &seat,
        desk.pressed.expect("waited for"),
        ResizeEdge::BottomRight,
    );
    until(&mut queue, &mut desk, |desk| desk.pointed.is_none());
    session.command(Command::Motion { by: (20, 20) });
    until(&mut queue, &mut desk, |desk| desk.resizing);

    unmap(&mut desk, &mut queue, 1);
    session.wait_for(|facts| facts.surface(page).is_none());
    let told = configures(&desk, 1);
    session.command(Command::Motion { by: (20, 20) });
    session.command(Command::Release {
        button: Button::Left,
    });
    // The resize over, the pointer is the window's beneath it once it moves.
    session.command(Command::Motion { by: (-1, -1) });
    let below = desk.windows[0].wl_surface().clone();
    until(&mut queue, &mut desk, |desk| {
        desk.pointed.as_ref() == Some(&below)
    });
    assert_eq!(
        configures(&desk, 1),
        told,
        "the resize went on configuring the window its client unmapped: {:?}",
        desk.said_to(desk.windows[1].wl_surface())
    );

    session.stop((desk, queue));
}
