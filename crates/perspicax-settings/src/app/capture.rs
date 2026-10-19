//! The capture channel: `perspicax-settings-v1`, on a Wayland connection of
//! the app's own, beside the one winit draws the window on.
//!
//! The compositor captures for the process whose surface has the keyboard,
//! so winit's window counts, and this connection needs no surface of its
//! own. A thread does nothing but dispatch its events, each sent to the page
//! over a channel and followed by a repaint. The page asks from its own
//! thread, straight on the proxy, which any thread may use.

use std::sync::mpsc::{self, Receiver, Sender};

use perspicax_policy::{Hatch, KeyPress, Keysym, Mods};
use perspicax_protocols::settings::v1::client::{
    perspicax_chord_capture_v1::{self, Event, PerspicaxChordCaptureV1, Reason},
    perspicax_settings_v1::PerspicaxSettingsV1,
};
use perspicax_settings::Pressed;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

/// What a capture heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Heard {
    Pressed(Pressed),
    /// The Logo key, tapped on its own.
    Tap,
    /// An escape hatch, which the compositor did once it had said so.
    Hatch(Hatch),
    /// Nothing was captured, for this reason.
    Cancelled(Why),
}

/// Why a capture ended with nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// The window lost the keyboard, or never had it.
    Unfocused,
    Locked,
    /// Another capture was asked for, by another program.
    Replaced,
    /// `[protocols] settings` no longer admits this program.
    Withdrawn,
    /// A reason this app is too old to know.
    Unknown,
}

/// Where the page asks for a chord, and hears it: the compositor, or a
/// test standing in for it.
pub(crate) trait Channel {
    /// Wait for the next chord pressed, giving up any capture waiting.
    fn ask(&mut self);
    /// Stop waiting.
    fn stop(&mut self);
    /// What the capture asked for last has heard, once it has.
    fn heard(&mut self) -> Option<Heard>;
}

/// Why there is no capture: the session offers none to this program.
pub(crate) const NOT_OFFERED: &str = "This session offers no chord capture to this program. \
    perspicax's `[protocols] settings` rule admits `perspicax-settings` by name; \
    type the chord instead.";

/// The compositor's channel.
pub(crate) struct Wayland {
    connection: Connection,
    settings: PerspicaxSettingsV1,
    queue: QueueHandle<Listener>,
    heard: Receiver<(u64, Heard)>,
    /// The capture waiting, by the number it was asked under.
    waiting: Option<(u64, PerspicaxChordCaptureV1)>,
    asked: u64,
}

impl Wayland {
    /// Connect, and start the thread that hears. `woken` is called on that
    /// thread after each thing heard: the window's repaint.
    ///
    /// # Errors
    ///
    /// Why there is no capture, as the page says it.
    pub(crate) fn connect(woken: impl Fn() + Send + 'static) -> Result<Self, String> {
        let connection = Connection::connect_to_env().map_err(|error| {
            format!("No Wayland session to capture a chord in ({error}); type the chord instead.")
        })?;
        let (globals, mut queue) =
            registry_queue_init::<Listener>(&connection).map_err(|error| {
                format!(
                    "The session's globals could not be read ({error}); type the chord instead."
                )
            })?;
        let handle = queue.handle();
        let settings = globals
            .bind::<PerspicaxSettingsV1, _, _>(&handle, 1..=1, ())
            .map_err(|_| NOT_OFFERED.to_owned())?;
        let (tell, heard) = mpsc::channel();
        let mut listener = Listener {
            tell,
            woken: Box::new(woken),
        };
        std::thread::Builder::new()
            .name("capture".to_owned())
            .spawn(move || while queue.blocking_dispatch(&mut listener).is_ok() {})
            .map_err(|error| {
                format!("The capture could not start ({error}); type the chord instead.")
            })?;
        Ok(Self {
            connection,
            settings,
            queue: handle,
            heard,
            waiting: None,
            asked: 0,
        })
    }
}

impl Channel for Wayland {
    fn ask(&mut self) {
        self.stop();
        self.asked += 1;
        let capture = self.settings.capture_chord(&self.queue, self.asked);
        self.waiting = Some((self.asked, capture));
        let _ = self.connection.flush();
    }

    fn stop(&mut self) {
        if let Some((_, capture)) = self.waiting.take() {
            capture.destroy();
            let _ = self.connection.flush();
        }
    }

    fn heard(&mut self) -> Option<Heard> {
        let asked = self.waiting.as_ref()?.0;
        let heard = latest(&self.heard, asked)?;
        self.waiting = None;
        Some(heard)
    }
}

/// What the capture asked for as `asked` heard, if it has. What an earlier
/// one heard before it was given up is not this one's to hear, and goes.
fn latest(heard: &Receiver<(u64, Heard)>, asked: u64) -> Option<Heard> {
    heard
        .try_iter()
        .find_map(|(number, heard)| (number == asked).then_some(heard))
}

/// The thread's side: each event, sent to the page.
struct Listener {
    tell: Sender<(u64, Heard)>,
    woken: Box<dyn Fn() + Send>,
}

impl Dispatch<PerspicaxChordCaptureV1, u64> for Listener {
    fn event(
        listener: &mut Self,
        capture: &PerspicaxChordCaptureV1,
        event: Event,
        asked: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Each event is the last: the client destroys what has heard.
        capture.destroy();
        if let Some(heard) = heard(event) {
            let _ = listener.tell.send((*asked, heard));
            (listener.woken)();
        }
    }
}

impl Dispatch<PerspicaxSettingsV1, ()> for Listener {
    fn event(
        _: &mut Self,
        _: &PerspicaxSettingsV1,
        _: <PerspicaxSettingsV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Listener {
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

/// What `event` says, as the page hears it. `None` for an event this app is
/// too old to know.
fn heard(event: Event) -> Option<Heard> {
    Some(match event {
        Event::Chord {
            mods,
            made,
            raw,
            latin,
            shifted,
        } => Heard::Pressed(perspicax_settings::pressed(&press(
            mods, made, &raw, latin, shifted,
        ))),
        Event::Tap => Heard::Tap,
        Event::Hatch {
            kind: WEnum::Value(perspicax_chord_capture_v1::Hatch::Exit),
            ..
        } => Heard::Hatch(Hatch::Exit),
        Event::Hatch { vt, .. } => Heard::Hatch(Hatch::Vt(u8::try_from(vt).unwrap_or(u8::MAX))),
        Event::Cancelled { reason } => Heard::Cancelled(match reason {
            WEnum::Value(Reason::Unfocused) => Why::Unfocused,
            WEnum::Value(Reason::Locked) => Why::Locked,
            WEnum::Value(Reason::Replaced) => Why::Replaced,
            WEnum::Value(Reason::Withdrawn) => Why::Withdrawn,
            _ => Why::Unknown,
        }),
        _ => return None,
    })
}

/// The press a chord event describes: `raw` is keysyms as 32-bit numbers in
/// this host's byte order, and 0 is no keysym.
fn press(
    mods: WEnum<perspicax_chord_capture_v1::Mods>,
    made: u32,
    raw: &[u8],
    latin: u32,
    shifted: u32,
) -> KeyPress {
    let some = |sym: u32| (sym != 0).then(|| Keysym::new(sym));
    KeyPress {
        mods: held(mods),
        made: Keysym::new(made),
        raw: raw
            .chunks_exact(4)
            .map(|bytes| Keysym::new(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
            .collect(),
        latin: some(latin),
        shifted: some(shifted),
    }
}

/// The modifiers, as the protocol sent them; a bit this app does not know
/// is no modifier a binding names.
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

#[cfg(test)]
mod tests {
    use perspicax_config::Trigger;
    use perspicax_policy::Chord;

    use super::*;

    fn wire(syms: &[Keysym]) -> Vec<u8> {
        syms.iter()
            .flat_map(|sym| sym.raw().to_ne_bytes())
            .collect()
    }

    #[test]
    fn the_wire_is_read_in_the_hosts_byte_order() {
        use perspicax_chord_capture_v1::Mods as Wire;
        let event = Event::Chord {
            mods: WEnum::Value(Wire::Logo | Wire::Shift),
            made: Keysym::Return.raw(),
            raw: wire(&[Keysym::Return, Keysym::KP_Enter]),
            latin: 0,
            shifted: 0,
        };
        let logo_shift = Mods {
            logo: true,
            shift: true,
            ..Mods::default()
        };
        assert_eq!(
            heard(event),
            Some(Heard::Pressed(Pressed::Chord {
                trigger: Trigger::Chord(Chord {
                    mods: logo_shift,
                    key: Keysym::Return,
                }),
                written: "Logo+Shift+Return".to_owned(),
            }))
        );
        let pressed = press(
            WEnum::Unknown(0x8 | 0x100),
            Keysym::q.raw(),
            &wire(&[Keysym::q, Keysym::Cyrillic_shorti]),
            Keysym::q.raw(),
            0,
        );
        assert_eq!(pressed.raw, [Keysym::q, Keysym::Cyrillic_shorti]);
        assert_eq!((pressed.latin, pressed.shifted), (Some(Keysym::q), None));
        assert_eq!(
            pressed.mods,
            Mods {
                logo: true,
                ..Mods::default()
            },
            "an unknown bit is no modifier"
        );
    }

    #[test]
    fn what_a_capture_given_up_heard_is_not_the_next_ones() {
        let (tell, heard) = mpsc::channel();
        tell.send((1, Heard::Tap)).unwrap();
        assert_eq!(latest(&heard, 2), None, "the first capture's tap");
        tell.send((1, Heard::Cancelled(Why::Replaced))).unwrap();
        tell.send((2, Heard::Pressed(Pressed::GaveUp))).unwrap();
        assert_eq!(latest(&heard, 2), Some(Heard::Pressed(Pressed::GaveUp)));
        assert_eq!(latest(&heard, 2), None, "each is heard once");
    }

    #[test]
    fn a_tap_a_hatch_and_a_cancel_are_each_heard_as_what_they_are() {
        use perspicax_chord_capture_v1::Hatch as Wire;
        assert_eq!(heard(Event::Tap), Some(Heard::Tap));
        assert_eq!(
            heard(Event::Hatch {
                kind: WEnum::Value(Wire::Vt),
                vt: 2,
            }),
            Some(Heard::Hatch(Hatch::Vt(2)))
        );
        assert_eq!(
            heard(Event::Hatch {
                kind: WEnum::Value(Wire::Exit),
                vt: 0,
            }),
            Some(Heard::Hatch(Hatch::Exit))
        );
        assert_eq!(
            heard(Event::Cancelled {
                reason: WEnum::Value(Reason::Locked),
            }),
            Some(Heard::Cancelled(Why::Locked))
        );
        assert_eq!(
            heard(Event::Cancelled {
                reason: WEnum::Unknown(9),
            }),
            Some(Heard::Cancelled(Why::Unknown))
        );
    }
}
