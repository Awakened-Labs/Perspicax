//! The monitors, as a display tool sees and rearranges them, by a
//! compositor with two virtual ones: `wlr-output-management-unstable-v1`, as
//! wlr-randr and kanshi speak it.
//!
//! Both monitors are listed with their mode, place and scale. A virtual
//! monitor can be given any size, moved, scaled and turned off, and the desk
//! the facts describe follows. Turning every monitor off is refused, a
//! configuration built against a desk that has since changed is cancelled,
//! and while the session is locked nothing is applied.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use common::{Session, until};
use perspicax_compositor::{Backend, Virtual};
use perspicax_node::Rect;
use perspicax_policy::{Access, Place, Shape, Side};

fn session(name: &str) -> Session {
    Session::start(
        name,
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
        },
    )
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_display_tool_sees_both_monitors_where_they_are() {
    let session = session("displays-list");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_displays(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.display_serial.is_some());

    assert_eq!(desk.shown.len(), 2);
    let first = desk.head_named("HEADLESS-1");
    assert!(first.enabled);
    assert_eq!(first.position, (0, 0));
    assert_eq!(desk.current_size("HEADLESS-1"), Some((1280, 1024)));
    assert_eq!(desk.head_named("HEADLESS-2").position, (1280, 0));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_virtual_monitor_takes_any_size_and_the_desk_follows() {
    let session = session("displays-mode");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_displays(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.display_serial.is_some());

    let configuration = desk.configuration(&qh, None);
    desk.enable(&configuration, "HEADLESS-1", &qh)
        .set_custom_mode(1024, 768, 60_000);
    desk.enable(&configuration, "HEADLESS-2", &qh);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("succeeded"));

    until(&mut queue, &mut desk, |desk| {
        desk.current_size("HEADLESS-1") == Some((1024, 768))
            && desk.head_named("HEADLESS-2").position == (1024, 0)
    });
    session.wait_for(|facts| facts.outputs().first() == Some(&Rect::new(0.0, 0.0, 1024.0, 768.0)));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_monitor_can_be_moved_scaled_and_turned_off_and_on() {
    let session = session("displays-place");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_displays(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.display_serial.is_some());

    // Under the first, at scale 2: 960x540 logical.
    let configuration = desk.configuration(&qh, None);
    desk.enable(&configuration, "HEADLESS-1", &qh)
        .set_position(0, 0);
    let second = desk.enable(&configuration, "HEADLESS-2", &qh);
    second.set_position(0, 1024);
    second.set_scale(2.0);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("succeeded"));
    session.wait_for(|facts| {
        facts
            .outputs()
            .contains(&Rect::new(0.0, 1024.0, 960.0, 1564.0))
    });

    // Off, and the desk is one monitor.
    until(&mut queue, &mut desk, |desk| {
        desk.head_named("HEADLESS-2").scale == 2.0
    });
    let configuration = desk.configuration(&qh, None);
    desk.enable(&configuration, "HEADLESS-1", &qh);
    configuration.disable_head(&desk.head_named("HEADLESS-2").head);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("succeeded"));
    session.wait_for(|facts| facts.outputs().len() == 1);
    until(&mut queue, &mut desk, |desk| {
        !desk.head_named("HEADLESS-2").enabled
    });

    // And on again.
    let configuration = desk.configuration(&qh, None);
    desk.enable(&configuration, "HEADLESS-1", &qh);
    desk.enable(&configuration, "HEADLESS-2", &qh);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("succeeded"));
    session.wait_for(|facts| facts.outputs().len() == 2);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn turning_every_monitor_off_fails_and_changes_nothing() {
    let session = session("displays-none");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_displays(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.display_serial.is_some());

    let configuration = desk.configuration(&qh, None);
    configuration.disable_head(&desk.head_named("HEADLESS-1").head);
    configuration.disable_head(&desk.head_named("HEADLESS-2").head);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("failed"));
    assert_eq!(session.wait_for(|_| true).outputs().len(), 2);

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_configuration_for_a_desk_that_has_changed_is_cancelled() {
    let session = session("displays-stale");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_displays(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.display_serial.is_some());
    let stale = desk.display_serial.unwrap().wrapping_sub(1);

    let configuration = desk.configuration(&qh, Some(stale));
    desk.enable(&configuration, "HEADLESS-1", &qh);
    desk.enable(&configuration, "HEADLESS-2", &qh);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("cancelled"));

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn nothing_is_applied_while_locked() {
    let session = session("displays-locked");
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.bind_displays(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.display_serial.is_some());

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    let configuration = desk.configuration(&qh, None);
    desk.enable(&configuration, "HEADLESS-1", &qh)
        .set_custom_mode(800, 600, 60_000);
    desk.enable(&configuration, "HEADLESS-2", &qh);
    configuration.apply();
    until(&mut queue, &mut desk, |desk| desk.configured.is_some());
    assert_eq!(desk.configured, Some("failed"));
    assert_eq!(
        session.wait_for(|_| true).outputs().first(),
        Some(&Rect::new(0.0, 0.0, 1280.0, 1024.0))
    );

    desk.unlock();
    session.stop((desk, queue));
}
