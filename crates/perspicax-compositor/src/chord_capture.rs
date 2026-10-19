//! `perspicax-settings-v1`: the next chord pressed, for the settings app to
//! bind.
//!
//! A binding is matched on the seat's key path, before any client sees the
//! key, so an app listening on its own keyboard would never hear a chord
//! already bound: pressing Logo+Return to rebind it would start a terminal.
//! The app asks here instead. The key path takes the next chord from the
//! bindings and the client alike ([`perspicax_policy::take`]) and tells it
//! here, with the keysyms a binding is matched by
//! ([`perspicax_policy::KeyPress`]). A tap of the Logo key is told as a tap,
//! and an escape hatch as itself, and still done.
//!
//! One capture waits at a time, whoever asked, and only while a surface of
//! the asking process holds the keyboard: the process, because the app's
//! toolkit keeps a connection of its own, and the capture is asked for on
//! another. A capture ends with what was pressed, or is cancelled: when the
//! keyboard goes to another process or to nothing, when the session locks,
//! when another capture is asked for, and when a reload's
//! `[protocols] settings` rule no longer admits the client.
//!
//! Gated like the shell's channel: offered only to the programs the rule
//! admits. Only a seat has a person's keys; headless, a capture waits until
//! it is cancelled.

use perspicax_node::Origin;
use perspicax_policy::{Hatch, KeyPress, Mods, Protocol};
pub(crate) use perspicax_protocols::settings::v1::server::perspicax_chord_capture_v1::Reason;
use perspicax_protocols::settings::v1::server::{
    perspicax_chord_capture_v1::{self, PerspicaxChordCaptureV1},
    perspicax_settings_v1::{self, PerspicaxSettingsV1},
};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
    protocol::wl_surface::WlSurface,
};

use crate::{
    access::{Filtered, Gate},
    state::{ClientState, Compositor},
};

/// The capture waiting, if one is.
#[derive(Default)]
pub(crate) struct Captures {
    waiting: Option<Waiting>,
}

struct Waiting {
    capture: PerspicaxChordCaptureV1,
    /// The process that asked, whose surfaces hold the keyboard.
    pid: u32,
}

/// How a capture ended.
pub(crate) enum Heard<'a> {
    /// A chord, taken from the bindings and the client.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "the seat's keys"))]
    Chord(&'a KeyPress),
    /// A tap of the Logo key.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "the seat's keys"))]
    Tap,
    /// An escape hatch, done as soon as it is told.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "the seat's keys"))]
    Hatch(Hatch),
    /// Nothing.
    Cancelled(Reason),
}

impl Captures {
    /// Advertise the global, filtered by `gate`.
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, PerspicaxSettingsV1, _>(
            1,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::Settings,
            },
        );
        Self::default()
    }
}

impl Compositor {
    /// Whether a capture waits for the next chord.
    #[cfg_attr(not(feature = "seat"), expect(dead_code, reason = "the seat's keys"))]
    pub(crate) fn capturing(&self) -> bool {
        self.captures.waiting.is_some()
    }

    /// End the waiting capture, if one waits, telling it how.
    pub(crate) fn end_capture(&mut self, heard: Heard<'_>) {
        let Some(Waiting { capture, .. }) = self.captures.waiting.take() else {
            return;
        };
        match heard {
            Heard::Chord(press) => capture.chord(
                wire(press.mods),
                press.made.raw(),
                press
                    .raw
                    .iter()
                    .flat_map(|sym| sym.raw().to_ne_bytes())
                    .collect(),
                press.latin.map_or(0, |sym| sym.raw()),
                press.shifted.map_or(0, |sym| sym.raw()),
            ),
            Heard::Tap => capture.tap(),
            Heard::Hatch(Hatch::Exit) => capture.hatch(perspicax_chord_capture_v1::Hatch::Exit, 0),
            Heard::Hatch(Hatch::Vt(vt)) => {
                capture.hatch(perspicax_chord_capture_v1::Hatch::Vt, u32::from(vt));
            }
            Heard::Cancelled(reason) => {
                tracing::debug!(?reason, "chord capture cancelled");
                capture.cancelled(reason);
            }
        }
    }

    /// The keyboard went to `focused`: a capture whose process no longer
    /// holds it is cancelled. Called from inside smithay's `set_focus`, so
    /// the keyboard itself is not asked.
    pub(crate) fn capture_follows_keyboard(&mut self, focused: Option<&WlSurface>) {
        let Some(waiting) = &self.captures.waiting else {
            return;
        };
        if focused.and_then(|surface| self.pid_of_surface(surface)) != Some(waiting.pid) {
            self.end_capture(Heard::Cancelled(Reason::Unfocused));
        }
    }

    /// Cancel the capture of a client the rules no longer admit.
    pub(crate) fn revoke_settings(&mut self) {
        let Some(waiting) = &self.captures.waiting else {
            return;
        };
        let admitted = self
            .display
            .get_client(waiting.capture.id())
            .is_ok_and(|client| self.gate.admits(Protocol::Settings, &client));
        if !admitted {
            tracing::info!("a chord capture withdrawn from a client [protocols] no longer admits");
            self.end_capture(Heard::Cancelled(Reason::Withdrawn));
        }
    }

    /// A client asked for the next chord.
    fn ask_capture(&mut self, client: &Client, capture: PerspicaxChordCaptureV1) {
        if self.lock.is_some() {
            capture.cancelled(Reason::Locked);
            return;
        }
        if !self.gate.admits(Protocol::Settings, client) {
            capture.cancelled(Reason::Withdrawn);
            return;
        }
        self.end_capture(Heard::Cancelled(Reason::Replaced));
        let asking = pid_of(client);
        let holding = self
            .keyboard_focus()
            .and_then(|surface| self.pid_of_surface(&surface));
        match asking.filter(|&pid| holding == Some(pid)) {
            Some(pid) => self.captures.waiting = Some(Waiting { capture, pid }),
            None => capture.cancelled(Reason::Unfocused),
        }
    }

    /// The process whose client drew `surface`, if it was attested.
    fn pid_of_surface(&self, surface: &WlSurface) -> Option<u32> {
        let client = self.display.get_client(surface.id()).ok()?;
        pid_of(&client)
    }
}

/// The process `client` runs in, as attested when it connected.
fn pid_of(client: &Client) -> Option<u32> {
    match client.get_data::<ClientState>()?.origin() {
        Origin::Process(process) => Some(process.pid),
        Origin::Unattributed | Origin::X11(_) => None,
    }
}

/// The modifiers, as the protocol sends them.
fn wire(mods: Mods) -> perspicax_chord_capture_v1::Mods {
    use perspicax_chord_capture_v1::Mods as Wire;
    [
        (mods.ctrl, Wire::Ctrl),
        (mods.alt, Wire::Alt),
        (mods.shift, Wire::Shift),
        (mods.logo, Wire::Logo),
    ]
    .into_iter()
    .filter(|&(held, _)| held)
    .fold(Wire::empty(), |wire, (_, bit)| wire | bit)
}

impl GlobalDispatch<PerspicaxSettingsV1, Filtered> for Compositor {
    fn bind(
        _state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<PerspicaxSettingsV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<PerspicaxSettingsV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        _settings: &PerspicaxSettingsV1,
        request: perspicax_settings_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        // The other request is `destroy`, which leaves a capture waiting.
        if let perspicax_settings_v1::Request::CaptureChord { id } = request {
            let capture = data_init.init(id, ());
            state.ask_capture(client, capture);
        }
    }
}

impl Dispatch<PerspicaxChordCaptureV1, ()> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _capture: &PerspicaxChordCaptureV1,
        _request: perspicax_chord_capture_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Its one request is `destroy`, which `destroyed` hears.
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        capture: &PerspicaxChordCaptureV1,
        _data: &(),
    ) {
        state
            .captures
            .waiting
            .take_if(|waiting| waiting.capture == *capture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_modifier_has_its_own_bit_on_the_wire() {
        use perspicax_chord_capture_v1::Mods as Wire;
        let alone = |set: fn(&mut Mods)| {
            let mut mods = Mods::default();
            set(&mut mods);
            wire(mods)
        };
        assert_eq!(alone(|mods| mods.ctrl = true), Wire::Ctrl);
        assert_eq!(alone(|mods| mods.alt = true), Wire::Alt);
        assert_eq!(alone(|mods| mods.shift = true), Wire::Shift);
        assert_eq!(alone(|mods| mods.logo = true), Wire::Logo);
        assert_eq!(wire(Mods::default()), Wire::empty());
        let all = Mods {
            ctrl: true,
            alt: true,
            shift: true,
            logo: true,
        };
        assert_eq!(wire(all).bits(), 0b1111, "as the protocol numbers them");
    }
}
