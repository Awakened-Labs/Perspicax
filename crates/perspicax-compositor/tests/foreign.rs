//! A dialog one program opens for another program's window: xdg-foreign,
//! v2, as the desktop portal speaks it (#235).
//!
//! Firefox asks the portal for a file chooser: it exports its window, hands
//! the portal the handle it is given, and the portal imports it and makes it
//! its dialog's parent. Both programs find the protocol. A taskbar is told
//! the dialog hangs from the program's window, as it is told of a program's
//! own dialogs. A surface that is not a window is refused as it is exported,
//! ending the program that exported it, as the protocol says, rather than
//! the portal that would later be handed it. A handle that names nothing
//! leaves the portal's dialog without a parent, and the portal running.
//!
//! Being kept on top with the window is in `on_top.rs`, and taking the keys
//! with it in `withheld.rs`. Like the other live tests these bind a real
//! Wayland socket, so they need `XDG_RUNTIME_DIR`, and are `#[ignore]`d for
//! `ci/live-tests.sh` to run.

mod common;

use common::{Session, advertised, hand_over, refused, until};
use perspicax_compositor::Backend;
use wayland_client::Proxy as _;

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_finds_xdg_foreign() {
    let session = Session::start("foreign-offered", Backend::headless((800, 600)));
    let (desk, queue, _qh, globals) = session.client();
    assert!(advertised(&globals, "zxdg_exporter_v2"));
    assert!(advertised(&globals, "zxdg_importer_v2"));
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_taskbar_is_told_a_dialog_of_another_program_hangs_from_its_window() {
    let session = Session::start("foreign-taskbar", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.bind_foreign(&globals, &qh);
    desk.open_window(&qh, "window", "firefox");
    until(&mut queue, &mut desk, |desk| desk.task("window").is_some());
    let (_exported, handle) = hand_over(&mut queue, &mut desk, &qh, 0);

    let (mut portal, mut portal_queue, portal_qh, portal_globals) = session.client();
    portal.bind_foreign(&portal_globals, &portal_qh);
    let imported = portal.import(&portal_qh, &handle);
    portal.open_dialog_for(&portal_qh, "dialog", "portal", &imported);
    until(&mut portal_queue, &mut portal, |portal| portal.drawn == 1);

    until(&mut queue, &mut desk, |desk| {
        desk.task("dialog")
            .is_some_and(|task| task.parent.is_some())
    });
    let window = desk.task("window").expect("the window").handle.id();
    assert_eq!(
        desk.task("dialog").and_then(|task| task.parent.clone()),
        Some(window),
        "the program's window, by the handle the taskbar holds on it"
    );

    session.stop(((desk, queue), (portal, portal_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn exporting_a_surface_that_is_not_a_window_is_refused() {
    let session = Session::start("foreign-not-a-window", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_foreign(&globals, &qh);
    let surface = desk.surface(&qh);
    desk.export_surface(&qh, &surface);
    assert_eq!(
        refused(&mut queue, &mut desk),
        ("zxdg_exporter_v2".to_owned(), 0),
        "invalid_surface, on the program that exported it"
    );
    assert!(!session.ended(), "the compositor is still running");
    assert!(desk.handles.is_empty(), "no handle was made to hand on");

    // Another program goes on as before.
    let (mut other, mut other_queue, other_qh, _) = session.client();
    other.open_window(&other_qh, "other", "other");
    until(&mut other_queue, &mut other, |other| other.drawn == 1);

    session.stop(((desk, queue), (other, other_queue)));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_handle_that_names_nothing_leaves_the_dialog_without_a_parent() {
    let session = Session::start("foreign-nothing", Backend::headless((800, 600)));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_taskbar(&globals, &qh);
    desk.open_window(&qh, "window", "firefox");
    until(&mut queue, &mut desk, |desk| desk.task("window").is_some());

    let (mut portal, mut portal_queue, portal_qh, portal_globals) = session.client();
    portal.bind_foreign(&portal_globals, &portal_qh);
    let imported = portal.import(&portal_qh, "no such handle");
    until(&mut portal_queue, &mut portal, |portal| {
        portal.imports_gone.contains(&imported.id())
    });
    portal.open_dialog_for(&portal_qh, "dialog", "portal", &imported);
    until(&mut portal_queue, &mut portal, |portal| portal.drawn == 1);

    until(&mut queue, &mut desk, |desk| desk.task("dialog").is_some());
    assert_eq!(
        desk.task("dialog").and_then(|task| task.parent.clone()),
        None,
        "it hangs from nothing"
    );

    session.stop(((desk, queue), (portal, portal_queue)));
}
