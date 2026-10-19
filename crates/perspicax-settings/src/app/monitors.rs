//! The monitors, as the session lists them to a display tool:
//! `wlr-output-management-unstable-v1`, on a Wayland connection of the
//! app's own, read and never asked of.
//!
//! The Outputs page shows each connected monitor, the modes it offers and
//! where it is now. What is changed is written to the file, as every other
//! setting is, and the session puts the file in force: so nothing here
//! sends a configuration. A thread does nothing but dispatch, and each time
//! the session says it has told everything (`done`), the monitors as told
//! go to the page over a channel, followed by a repaint.

use std::sync::mpsc::{self, Receiver, Sender};

use perspicax_policy::{Head, HeadMode};
use wayland_client::{
    Connection, Dispatch, QueueHandle, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

/// One connected monitor: as a display tool sees it, and who made it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Monitor {
    pub(crate) head: Head,
    /// Its make and model, as it says them: `Dell U2720Q`. Empty for a
    /// monitor that says neither, or one that is dark.
    pub(crate) made: String,
}

/// What the session last said.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Seen {
    /// Every monitor connected, lit or dark, in the session's order.
    Monitors(Vec<Monitor>),
    /// The session stopped listing them: `[protocols] output-management`
    /// no longer admits this program.
    Withdrawn,
}

/// Where the page hears of the monitors: the session, or a test standing in
/// for it.
pub(crate) trait Monitors {
    /// What the session said since this was last asked, the latest only.
    fn seen(&mut self) -> Option<Seen>;
}

/// Why there is no list of monitors: the session offers none to this
/// program.
pub(crate) const NOT_OFFERED: &str = "This session lists no monitors to this program: \
    perspicax's `[protocols] output-management` rule leaves out `perspicax-settings`. \
    Each rule here is the file's, and a mode is typed.";

/// Why the list went: the rule narrowed while the app was open.
pub(crate) const WITHDRAWN: &str = "The session stopped listing monitors to this program: \
    `[protocols] output-management` no longer admits `perspicax-settings`. Each rule here is \
    the file's, and a mode is typed.";

/// The session's list.
pub(crate) struct Wayland {
    heard: Receiver<Seen>,
    // Held so the connection, and the list it is told, last as long as the
    // page does.
    _connection: Connection,
    _manager: ZwlrOutputManagerV1,
}

impl Wayland {
    /// Connect, and start the thread that hears. `woken` is called on that
    /// thread after each list: the window's repaint.
    ///
    /// # Errors
    ///
    /// Why there is no list, as the page says it.
    pub(crate) fn connect(woken: impl Fn() + Send + 'static) -> Result<Self, String> {
        let unlisted = |error: &dyn std::fmt::Display| {
            format!("No monitors could be listed ({error}). Each rule here is the file's.")
        };
        let connection = Connection::connect_to_env().map_err(|error| unlisted(&error))?;
        let (globals, mut queue) =
            registry_queue_init::<Listener>(&connection).map_err(|error| unlisted(&error))?;
        let manager = globals
            .bind::<ZwlrOutputManagerV1, _, _>(&queue.handle(), 1..=4, ())
            .map_err(|_| NOT_OFFERED.to_owned())?;
        let (tell, heard) = mpsc::channel();
        let mut listener = Listener {
            tell,
            woken: Box::new(woken),
            heads: Vec::new(),
        };
        std::thread::Builder::new()
            .name("monitors".to_owned())
            .spawn(move || while queue.blocking_dispatch(&mut listener).is_ok() {})
            .map_err(|error| unlisted(&error))?;
        Ok(Self {
            heard,
            _connection: connection,
            _manager: manager,
        })
    }
}

impl Monitors for Wayland {
    fn seen(&mut self) -> Option<Seen> {
        self.heard.try_iter().last()
    }
}

/// The thread's side: each monitor as it is told, until `done`.
struct Listener {
    tell: Sender<Seen>,
    woken: Box<dyn Fn() + Send>,
    heads: Vec<Told>,
}

/// One monitor as told so far.
struct Told {
    object: ZwlrOutputHeadV1,
    name: String,
    make: String,
    model: String,
    enabled: bool,
    modes: Vec<(ZwlrOutputModeV1, HeadMode)>,
    current: Option<ZwlrOutputModeV1>,
    position: (i32, i32),
    scale: f64,
}

impl Listener {
    /// Every monitor, as told.
    fn monitors(&self) -> Vec<Monitor> {
        self.heads
            .iter()
            .map(|told| Monitor {
                head: Head {
                    name: told.name.clone(),
                    enabled: told.enabled,
                    modes: told.modes.iter().map(|(_, mode)| *mode).collect(),
                    current: told.current.as_ref().and_then(|current| {
                        told.modes.iter().position(|(mode, _)| mode == current)
                    }),
                    position: told.position,
                    scale: told.scale,
                },
                made: [told.make.as_str(), told.model.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" "),
            })
            .collect()
    }

    fn say(&self, seen: Seen) {
        let _ = self.tell.send(seen);
        (self.woken)();
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Listener {
    fn event(
        listener: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => listener.heads.push(Told {
                object: head,
                name: String::new(),
                make: String::new(),
                model: String::new(),
                enabled: false,
                modes: Vec::new(),
                current: None,
                position: (0, 0),
                scale: 1.0,
            }),
            zwlr_output_manager_v1::Event::Done { .. } => {
                listener.say(Seen::Monitors(listener.monitors()))
            }
            zwlr_output_manager_v1::Event::Finished => {
                listener.heads.clear();
                listener.say(Seen::Withdrawn);
            }
            _ => {}
        }
    }

    event_created_child!(Listener, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ())
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for Listener {
    fn event(
        listener: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_head_v1::Event;
        if let Event::Finished = event {
            listener.heads.retain(|told| told.object != *head);
            return;
        }
        let Some(told) = listener.heads.iter_mut().find(|told| told.object == *head) else {
            return;
        };
        match event {
            Event::Name { name } => told.name = name,
            Event::Make { make } => told.make = make,
            Event::Model { model } => told.model = model,
            Event::Mode { mode } => told.modes.push((
                mode,
                HeadMode {
                    width: 0,
                    height: 0,
                    refresh: 0,
                    preferred: false,
                },
            )),
            Event::Enabled { enabled } => told.enabled = enabled != 0,
            Event::CurrentMode { mode } => told.current = Some(mode),
            Event::Position { x, y } => told.position = (x, y),
            Event::Scale { scale } => told.scale = scale,
            _ => {}
        }
    }

    event_created_child!(Listener, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ())
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for Listener {
    fn event(
        listener: &mut Self,
        object: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        for told in &mut listener.heads {
            if let zwlr_output_mode_v1::Event::Finished = event {
                told.modes.retain(|(mode, _)| mode != object);
                continue;
            }
            let Some((_, mode)) = told.modes.iter_mut().find(|(mode, _)| mode == object) else {
                continue;
            };
            match event {
                zwlr_output_mode_v1::Event::Size { width, height } => {
                    (mode.width, mode.height) = (width, height);
                }
                zwlr_output_mode_v1::Event::Refresh { refresh } => mode.refresh = refresh,
                zwlr_output_mode_v1::Event::Preferred => mode.preferred = true,
                _ => {}
            }
            return;
        }
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
