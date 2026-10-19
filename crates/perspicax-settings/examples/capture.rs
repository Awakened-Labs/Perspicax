//! Capture chords through `perspicax-settings-v1`, as the settings app will,
//! and print each as the `[keys]` entry that binds it.
//!
//! A window of its own, since the compositor captures only for the process
//! whose surface has the keyboard. Click it, and it turns orange: the next
//! chord pressed is printed rather than done, so nothing starts and no menu
//! opens. A modifier on its own is still the window's, and Escape on its own
//! gives up, as the app's will. Between clicks the window is grey, and keys
//! are bound as ever: Logo+Space switches layout.
//!
//! The compositor offers the channel only to what `[protocols] settings`
//! admits, `perspicax-settings` by default, so it runs under that name:
//!
//! ```sh
//! cargo build --example capture -p perspicax-settings
//! cp target/debug/examples/capture /tmp/perspicax-settings
//! /tmp/perspicax-settings
//! ```
//!
//! The seat kit for #57 runs it so, and its checklist says what each chord
//! should print.

use perspicax_config::spell_press;
use perspicax_policy::{KeyPress, Keysym, Mods};
use perspicax_protocols::settings::v1::client::{
    perspicax_chord_capture_v1::{self, Event, Hatch, PerspicaxChordCaptureV1},
    perspicax_settings_v1::PerspicaxSettingsV1,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

const IDLE: u32 = 0xff40_4040;
const WAITING: u32 = 0xffc0_6a3c;

fn main() {
    let connection = Connection::connect_to_env().expect("a Wayland session");
    let (globals, mut queue) = registry_queue_init(&connection).expect("the registry");
    let qh = queue.handle();
    let settings = globals
        .bind::<PerspicaxSettingsV1, _, _>(&qh, 1..=1, ())
        .expect(
            "perspicax_settings_v1 is not offered: run this as `perspicax-settings`, \
             under a perspicax with the settings protocol",
        );
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let xdg_shell = XdgShell::bind(&globals, &qh).expect("xdg_wm_base");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm");
    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("perspicax-settings: click, then press a chord");
    window.set_app_id("perspicax-settings");
    window.set_min_size(Some((320, 160)));
    window.commit();
    let mut app = App {
        registry: RegistryState::new(&globals),
        seat: SeatState::new(&globals, &qh),
        output: OutputState::new(&globals, &qh),
        pool: SlotPool::new(480 * 240 * 4, &shm).expect("a pool"),
        shm,
        window,
        settings,
        pointer: None,
        capture: None,
        size: (480, 240),
        closed: false,
    };
    println!("Click the window, then press a chord. Escape alone gives up.");
    while !app.closed {
        queue
            .blocking_dispatch(&mut app)
            .expect("the compositor is there");
    }
}

struct App {
    registry: RegistryState,
    seat: SeatState,
    output: OutputState,
    shm: Shm,
    pool: SlotPool,
    window: Window,
    settings: PerspicaxSettingsV1,
    pointer: Option<wl_pointer::WlPointer>,
    /// The capture waiting, if one is.
    capture: Option<PerspicaxChordCaptureV1>,
    size: (u32, u32),
    closed: bool,
}

impl App {
    fn draw(&mut self) {
        let (width, height) = self.size;
        let colour = if self.capture.is_some() {
            WAITING
        } else {
            IDLE
        };
        let (buffer, canvas) = self
            .pool
            .create_buffer(
                i32::try_from(width).unwrap_or(i32::MAX),
                i32::try_from(height).unwrap_or(i32::MAX),
                i32::try_from(width * 4).unwrap_or(i32::MAX),
                wl_shm::Format::Argb8888,
            )
            .expect("a buffer");
        for pixel in canvas.chunks_exact_mut(4) {
            pixel.copy_from_slice(&colour.to_le_bytes());
        }
        let surface = self.window.wl_surface();
        buffer.attach_to(surface).expect("attach");
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        self.window.commit();
    }

    /// Wait for the next chord, unless one is already waited for.
    fn capture(&mut self, qh: &QueueHandle<Self>) {
        if self.capture.is_none() {
            self.capture = Some(self.settings.capture_chord(qh, ()));
            self.draw();
        }
    }
}

/// What a capture heard, as a person would be told it.
fn said(event: Event) -> String {
    match event {
        Event::Chord {
            mods,
            made,
            raw,
            latin,
            shifted,
        } => {
            let press = KeyPress {
                mods: held(mods),
                made: Keysym::new(made),
                raw: raw
                    .chunks_exact(4)
                    .map(|bytes| {
                        Keysym::new(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    })
                    .collect(),
                latin: Some(latin).filter(|&sym| sym != 0).map(Keysym::new),
                shifted: Some(shifted).filter(|&sym| sym != 0).map(Keysym::new),
            };
            if press.mods == Mods::default() && press.made == Keysym::Escape {
                return "Escape: given up, nothing bound".to_owned();
            }
            match spell_press(&press) {
                Some(entry) => format!("\"{entry}\""),
                None => format!("not bindable yet: {:?} with {:?}", press.made, press.mods),
            }
        }
        Event::Tap => "\"Logo\", a tap".to_owned(),
        Event::Hatch {
            kind: WEnum::Value(Hatch::Exit),
            ..
        } => "Ctrl+Alt+Backspace ends the session; no binding can take it".to_owned(),
        Event::Hatch { vt, .. } => {
            format!("Ctrl+Alt+F{vt} switches to VT {vt}; no binding can take it")
        }
        Event::Cancelled { reason } => format!("cancelled: {reason:?}"),
        _ => format!("{event:?}"),
    }
}

/// The modifiers, as the protocol sent them.
fn held(mods: WEnum<perspicax_chord_capture_v1::Mods>) -> Mods {
    use perspicax_chord_capture_v1::Mods as Wire;
    let bits = match mods {
        WEnum::Value(bits) => bits,
        WEnum::Unknown(bits) => Wire::from_bits_truncate(bits),
    };
    Mods {
        ctrl: bits.contains(Wire::Ctrl),
        alt: bits.contains(Wire::Alt),
        shift: bits.contains(Wire::Shift),
        logo: bits.contains(Wire::Logo),
    }
}

impl Dispatch<PerspicaxSettingsV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &PerspicaxSettingsV1,
        _: <PerspicaxSettingsV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<PerspicaxChordCaptureV1, ()> for App {
    fn event(
        app: &mut Self,
        capture: &PerspicaxChordCaptureV1,
        event: Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        println!("{}", said(event));
        capture.destroy();
        app.capture = None;
        app.draw();
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        if events
            .iter()
            .any(|event| matches!(event.kind, PointerEventKind::Press { .. }))
        {
            self.capture(qh);
        }
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        if let (Some(width), Some(height)) = configure.new_size {
            self.size = (width.get(), height.get());
        }
        self.draw();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_registry!(App);
