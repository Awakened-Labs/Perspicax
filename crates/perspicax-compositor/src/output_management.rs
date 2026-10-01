//! `wlr-output-management-unstable-v1`: the monitors, for a display tool to
//! read and rearrange.
//!
//! What wlr-randr, kanshi and wdisplays speak. Every monitor is a head, lit
//! or dark, with the modes it offers, where it is and at what scale. A tool
//! builds a configuration naming every head -- on at this mode, here, this
//! scale; or off -- and asks to test it or apply it. Nothing is changed until
//! the whole of it has been checked (`perspicax_policy::check_heads`), and a
//! configuration built against a desk that has since changed is cancelled
//! rather than applied to the wrong one.
//!
//! Fed like the other protocols that watch the desk: after every
//! publication of the facts, [`Compositor::sync_heads`] compares the heads
//! with what was last sent and, if they differ, tells every tool again under
//! a new serial. Nothing is applied while the session is locked, and who may
//! use it at all is `[protocols] output-management`.

use std::sync::{Arc, Mutex, PoisonError};

use perspicax_policy::{Head, HeadChange, ModeChoice, Protocol};
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::output_management::v1::server::{
            zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
            zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
            zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
            zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
            zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId,
        },
    },
};

use crate::{
    access::{Filtered, Gate},
    state::Compositor,
};

/// Version 4 adds adaptive sync, which is never on here and so never asked
/// about.
const VERSION: u32 = 4;

/// Every display tool, and what they were last told.
#[derive(Default)]
pub(crate) struct Displays {
    managers: Vec<Manager>,
    sent: Vec<Head>,
    serial: u32,
}

/// One tool's objects: a head per monitor, and its modes.
struct Manager {
    manager: ZwlrOutputManagerV1,
    heads: Vec<(String, ZwlrOutputHeadV1, Vec<ZwlrOutputModeV1>)>,
}

/// What a head object names.
#[derive(Debug, Clone)]
pub(crate) struct Named(String);

/// What a mode object is: which head, which of its modes.
#[derive(Debug, Clone)]
pub(crate) struct ModeOf {
    head: String,
    index: usize,
}

/// A configuration being built: the serial it was built against, and what
/// it says so far.
#[derive(Debug)]
pub(crate) struct Building {
    serial: u32,
    changes: Arc<Mutex<Vec<HeadChange>>>,
    used: Mutex<bool>,
}

/// One head's part of a configuration.
#[derive(Debug)]
pub(crate) struct Configuring {
    changes: Arc<Mutex<Vec<HeadChange>>>,
    head: String,
}

impl Displays {
    pub(crate) fn new(display: &DisplayHandle, gate: &Gate) -> Self {
        display.create_global::<Compositor, ZwlrOutputManagerV1, _>(
            VERSION,
            Filtered {
                gate: gate.clone(),
                protocol: Protocol::OutputManagement,
            },
        );
        Self::default()
    }
}

impl Compositor {
    /// Tell every display tool the monitors, if they changed.
    pub(crate) fn sync_heads(&mut self) {
        if self.lock.is_some() {
            return;
        }
        let now = self.heads();
        if now == self.displays.sent {
            return;
        }
        self.displays.serial = self.displays.serial.wrapping_add(1);
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let display = self.display.clone();
        let displays = &mut self.displays;
        for manager in &mut displays.managers {
            send(
                &display,
                manager,
                &displays.sent,
                &now,
                &outputs,
                displays.serial,
            );
        }
        displays.sent = now;
    }

    /// Withdraw from every client the rules no longer admit.
    pub(crate) fn revoke_displays(&mut self) {
        let gate = self.gate.clone();
        let display = self.display.clone();
        self.displays.managers.retain(|manager| {
            let admitted = display
                .get_client(manager.manager.id())
                .is_ok_and(|client| gate.admits(Protocol::OutputManagement, &client));
            if !admitted {
                manager.manager.finished();
                tracing::info!(
                    "output-management withdrawn from a client [protocols] no longer admits"
                );
            }
            admitted
        });
    }

    /// Test or apply a configuration.
    fn settle(
        &mut self,
        client: &Client,
        configuration: &ZwlrOutputConfigurationV1,
        building: &Building,
        apply: bool,
    ) {
        {
            let mut used = building.used.lock().unwrap_or_else(PoisonError::into_inner);
            if *used {
                configuration.post_error(
                    zwlr_output_configuration_v1::Error::AlreadyUsed,
                    "a configuration is applied or tested once",
                );
                return;
            }
            *used = true;
        }
        if building.serial != self.displays.serial {
            configuration.cancelled();
            return;
        }
        let changes = building
            .changes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let heads = self.heads();
        if let Some(missing) = heads
            .iter()
            .find(|head| !changes.iter().any(|change| change.name == head.name))
        {
            configuration.post_error(
                zwlr_output_configuration_v1::Error::UnconfiguredHead,
                format!("{} is neither enabled nor disabled", missing.name),
            );
            return;
        }
        if self.lock.is_some() || !self.gate.admits(Protocol::OutputManagement, client) {
            configuration.failed();
            return;
        }
        if let Err(rejection) = perspicax_policy::check_heads(&heads, &changes, self.custom_modes())
        {
            tracing::info!(?rejection, "output configuration refused");
            configuration.failed();
            return;
        }
        if apply {
            self.apply_heads(&changes);
        }
        configuration.succeeded();
    }
}

/// Bring one tool from `was` to `now`, then `done` under `serial`.
fn send(
    display: &DisplayHandle,
    manager: &mut Manager,
    was: &[Head],
    now: &[Head],
    outputs: &[Output],
    serial: u32,
) {
    let Ok(client) = display.get_client(manager.manager.id()) else {
        return;
    };
    manager.heads.retain(|(name, head, modes)| {
        let kept = now.iter().any(|now| now.name == *name);
        if !kept {
            for mode in modes {
                mode.finished();
            }
            head.finished();
        }
        kept
    });
    for head in now {
        let before = was.iter().find(|old| old.name == head.name);
        let known = manager
            .heads
            .iter_mut()
            .find(|(name, _, _)| *name == head.name);
        match known {
            Some((_, object, modes)) => {
                let mut changed = false;
                if before.is_none_or(|old| old.modes != head.modes) {
                    for mode in modes.drain(..) {
                        mode.finished();
                    }
                    *modes = announce_modes(display, &client, object, head);
                    changed = true;
                }
                if changed || before.is_none_or(|old| old != head) {
                    describe(object, modes, head, false);
                }
            }
            None => {
                let Ok(object) = client.create_resource::<ZwlrOutputHeadV1, Named, Compositor>(
                    display,
                    manager.manager.version(),
                    Named(head.name.clone()),
                ) else {
                    continue;
                };
                manager.manager.head(&object);
                object.name(head.name.clone());
                object.description(head.name.clone());
                if let Some(output) = outputs.iter().find(|output| output.name() == head.name) {
                    let physical = output.physical_properties();
                    object.physical_size(physical.size.w, physical.size.h);
                    if object.version() >= 2 {
                        object.make(physical.make);
                        object.model(physical.model);
                    }
                }
                let modes = announce_modes(display, &client, &object, head);
                describe(&object, &modes, head, true);
                manager.heads.push((head.name.clone(), object, modes));
            }
        }
    }
    manager.manager.done(serial);
}

fn announce_modes(
    display: &DisplayHandle,
    client: &Client,
    object: &ZwlrOutputHeadV1,
    head: &Head,
) -> Vec<ZwlrOutputModeV1> {
    head.modes
        .iter()
        .enumerate()
        .filter_map(|(index, mode)| {
            let created = client
                .create_resource::<ZwlrOutputModeV1, ModeOf, Compositor>(
                    display,
                    object.version().min(3),
                    ModeOf {
                        head: head.name.clone(),
                        index,
                    },
                )
                .ok()?;
            object.mode(&created);
            created.size(mode.width, mode.height);
            created.refresh(mode.refresh);
            if mode.preferred {
                created.preferred();
            }
            Some(created)
        })
        .collect()
}

/// A head's state: whether it is on, and, when it is, at which mode, where,
/// and at what scale.
fn describe(object: &ZwlrOutputHeadV1, modes: &[ZwlrOutputModeV1], head: &Head, first: bool) {
    object.enabled(i32::from(head.enabled));
    if head.enabled {
        if let Some(mode) = head.current.and_then(|index| modes.get(index)) {
            object.current_mode(mode);
        }
        object.position(head.position.0, head.position.1);
        if first {
            object.transform(smithay::utils::Transform::Normal.into());
        }
        object.scale(head.scale);
    }
}

impl GlobalDispatch<ZwlrOutputManagerV1, Filtered> for Compositor {
    fn bind(
        state: &mut Self,
        display: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _global: &Filtered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let mut manager = Manager {
            manager: data_init.init(resource, ()),
            heads: Vec::new(),
        };
        let outputs: Vec<Output> = state.space.outputs().cloned().collect();
        let displays = &state.displays;
        send(
            display,
            &mut manager,
            &[],
            &displays.sent,
            &outputs,
            displays.serial,
        );
        state.displays.managers.push(manager);
    }

    fn can_view(client: Client, global: &Filtered) -> bool {
        global.admits(&client)
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Compositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                data_init.init(
                    id,
                    Building {
                        serial,
                        changes: Arc::default(),
                        used: Mutex::new(false),
                    },
                );
            }
            zwlr_output_manager_v1::Request::Stop => {
                state
                    .displays
                    .managers
                    .retain(|held| held.manager != *manager);
                manager.finished();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, manager: &ZwlrOutputManagerV1, _data: &()) {
        state
            .displays
            .managers
            .retain(|held| held.manager != *manager);
    }
}

impl Dispatch<ZwlrOutputHeadV1, Named> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _head: &ZwlrOutputHeadV1,
        _request: zwlr_output_head_v1::Request,
        _data: &Named,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // Release is the only request, and `destroyed` does its work.
    }

    fn destroyed(state: &mut Self, _client: ClientId, head: &ZwlrOutputHeadV1, _data: &Named) {
        for manager in &mut state.displays.managers {
            manager.heads.retain(|(_, held, _)| held != head);
        }
    }
}

impl Dispatch<ZwlrOutputModeV1, ModeOf> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _mode: &ZwlrOutputModeV1,
        _request: zwlr_output_mode_v1::Request,
        _data: &ModeOf,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, Building> for Compositor {
    fn request(
        state: &mut Self,
        client: &Client,
        configuration: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        building: &Building,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_v1::Request;
        let name_twice = |name: &str| {
            let mut changes = building
                .changes
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if changes.iter().any(|change| change.name == name) {
                return true;
            }
            changes.push(HeadChange {
                name: name.to_owned(),
                enabled: true,
                mode: None,
                position: None,
                scale: None,
            });
            false
        };
        match request {
            Request::EnableHead { id, head } => {
                let name = head
                    .data::<Named>()
                    .map(|named| named.0.clone())
                    .unwrap_or_default();
                if name_twice(&name) {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyConfiguredHead,
                        format!("{name} is already in this configuration"),
                    );
                    return;
                }
                data_init.init(
                    id,
                    Configuring {
                        changes: Arc::clone(&building.changes),
                        head: name,
                    },
                );
            }
            Request::DisableHead { head } => {
                let name = head
                    .data::<Named>()
                    .map(|named| named.0.clone())
                    .unwrap_or_default();
                if name_twice(&name) {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyConfiguredHead,
                        format!("{name} is already in this configuration"),
                    );
                    return;
                }
                if let Some(change) = building
                    .changes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter_mut()
                    .find(|change| change.name == name)
                {
                    change.enabled = false;
                }
            }
            Request::Apply => state.settle(client, configuration, building, true),
            Request::Test => state.settle(client, configuration, building, false),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, Configuring> for Compositor {
    fn request(
        _state: &mut Self,
        _client: &Client,
        head: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        configuring: &Configuring,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_head_v1::{Error, Request};
        let mut changes = configuring
            .changes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(change) = changes
            .iter_mut()
            .find(|change| change.name == configuring.head)
        else {
            return;
        };
        let twice = |set: bool| {
            if set {
                head.post_error(Error::AlreadySet, "set twice in one configuration");
            }
            set
        };
        match request {
            Request::SetMode { mode } => {
                if twice(change.mode.is_some()) {
                    return;
                }
                match mode.data::<ModeOf>() {
                    Some(of) if of.head == configuring.head => {
                        change.mode = Some(ModeChoice::Listed(of.index));
                    }
                    _ => head.post_error(Error::InvalidMode, "a mode of another head"),
                }
            }
            Request::SetCustomMode {
                width,
                height,
                refresh,
            } => {
                if twice(change.mode.is_some()) {
                    return;
                }
                if width <= 0 || height <= 0 {
                    head.post_error(Error::InvalidCustomMode, "a mode of no size");
                    return;
                }
                change.mode = Some(ModeChoice::Custom {
                    width,
                    height,
                    refresh,
                });
            }
            Request::SetPosition { x, y } => {
                if !twice(change.position.is_some()) {
                    change.position = Some((x, y));
                }
            }
            Request::SetScale { scale } => {
                if !twice(change.scale.is_some()) {
                    change.scale = Some(scale);
                }
            }
            Request::SetTransform { transform } => {
                // Monitors here are only ever upright.
                if transform.into_result().ok() != Some(smithay::utils::Transform::Normal.into()) {
                    head.post_error(Error::InvalidTransform, "only normal is supported");
                }
            }
            // Adaptive sync is never on here, and a request about it is
            // ignored rather than honoured.
            _ => {}
        }
    }
}
