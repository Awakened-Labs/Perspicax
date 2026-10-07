//! `wp_cursor_shape_v1`: a client names the pointer it wants, and the
//! compositor draws it from the theme's cursor, so every application's
//! pointer is the same one.
//!
//! Headless has no monitor to draw a pointer on, so what this proves is the
//! protocol: the global is offered, and a shape named on it is taken rather
//! than refused. Which picture a shape becomes is checked by hand on
//! hardware; the pointer in a screen recording, which headless does draw, is
//! `tests/screencopy.rs`'s.

mod common;

use common::{Session, connect};
use perspicax_compositor::Backend;
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_pointer, wl_registry, wl_seat},
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{self, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};

/// A client that only names pointers.
struct Probe;

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_names_its_pointer_and_the_compositor_takes_it() {
    let session = Session::start("cursor-shape", Backend::headless((800, 600)));
    let connection = connect(session.socket());
    let (globals, mut queue) = registry_queue_init::<Probe>(&connection).expect("the registry");
    let qh = queue.handle();

    let manager: WpCursorShapeManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .expect("wp_cursor_shape_manager_v1 is offered");
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).expect("a seat");
    let pointer = seat.get_pointer(&qh, ());
    let device = manager.get_pointer(&pointer, &qh, ());
    device.set_shape(0, wp_cursor_shape_device_v1::Shape::Text);
    device.set_shape(0, wp_cursor_shape_device_v1::Shape::Grab);
    queue
        .roundtrip(&mut Probe)
        .expect("no protocol error: each shape was taken");

    device.destroy();
    manager.destroy();
    queue.roundtrip(&mut Probe).expect("and let go");
    session.stop((queue, connection));
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_pointer::WlPointer,
        _: wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpCursorShapeManagerV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeManagerV1,
        _: <WpCursorShapeManagerV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpCursorShapeDeviceV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &WpCursorShapeDeviceV1,
        _: <WpCursorShapeDeviceV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
