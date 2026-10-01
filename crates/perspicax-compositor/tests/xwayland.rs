//! An X11 window under Xwayland, and who the index is told drew it.
//!
//! The claim this checks is the provenance one. The compositor's Wayland
//! credentials name only Xwayland, so the X client's pid has to come from
//! somewhere else: first from the window's own `_NET_WM_PID`, a claim, then
//! from the X-Resource extension, the X server's own word. The facts must end
//! up saying `XRes`, with the server attested separately.
//!
//! The X client is this test process, speaking X11 through x11rb, so nothing
//! beyond `Xwayland` itself has to be installed. Needs `XDG_RUNTIME_DIR` and
//! `Xwayland` on the `PATH`, and is `#[ignore]`d for `ci/live-tests.sh`.

#![cfg(feature = "xwayland")]

use std::{
    thread,
    time::{Duration, Instant},
};

use perspicax_compositor::{Backend, Config, Facts, Requests, Stop};
use perspicax_node::{Origin, X11Basis};
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass},
    wrapper::ConnectionExt as _,
};

const TITLE: &str = "perspicax x11 provenance";

#[test]
#[ignore = "starts Xwayland on a real Wayland socket; needs XDG_RUNTIME_DIR and Xwayland"]
fn an_x11_window_is_attributed_to_its_client_through_xres() {
    let facts = Facts::new();
    let stop = Stop::new();
    let compositor = {
        let (facts, stop) = (facts.clone(), stop.clone());
        thread::spawn(move || {
            let config = Config {
                backend: Backend::headless((800, 600)),
                spawn: Vec::new(),
                env: Vec::new(),
                run_for: Some(Duration::from_secs(30)),
                config: None,
                socket: None,
                xwayland: true,
            };
            perspicax_compositor::run(&config, &facts, &Requests::new(), &stop)
        })
    };

    let display = eventually(Duration::from_secs(15), || facts.x11_display())
        .expect("Xwayland never became ready");
    let (x, screen) = x11rb::connect(Some(&format!(":{display}"))).expect("an X connection");
    let root = x.setup().roots[screen].root;
    let window = x.generate_id().expect("an X id");
    x.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        320,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().background_pixel(x.setup().roots[screen].white_pixel),
    )
    .expect("create_window");
    x.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        TITLE.as_bytes(),
    )
    .expect("WM_NAME");
    let net_wm_pid = x
        .intern_atom(false, b"_NET_WM_PID")
        .expect("intern")
        .reply()
        .expect("_NET_WM_PID")
        .atom;
    x.change_property32(
        PropMode::REPLACE,
        window,
        net_wm_pid,
        AtomEnum::CARDINAL,
        &[std::process::id()],
    )
    .expect("_NET_WM_PID");
    x.map_window(window).expect("map_window");
    x.flush().expect("flush");

    let described = eventually(Duration::from_secs(10), || {
        facts.read().surfaces().iter().find_map(|surface| {
            let Origin::X11(origin) = &surface.origin else {
                return None;
            };
            (surface.mapped
                && surface.title.as_deref() == Some(TITLE)
                && origin.basis == X11Basis::XRes)
                .then(|| (**origin).clone())
        })
    })
    .unwrap_or_else(|| panic!("never described with an XRes origin: {:?}", facts.read()));

    assert_eq!(
        described.client.map(|client| client.pid),
        Some(std::process::id()),
        "XRes names this test process as the window's client"
    );
    assert_ne!(
        described.server,
        std::process::id(),
        "and the attested server is Xwayland, a different process"
    );

    stop.request();
    compositor
        .join()
        .expect("the compositor thread panicked")
        .expect("the compositor failed");
}

fn eventually<T>(within: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(found) = probe() {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
