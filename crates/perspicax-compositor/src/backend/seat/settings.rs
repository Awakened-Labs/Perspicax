//! The person's config, applied to the seat: read at start, re-read on the
//! reload binding.
//!
//! A reload applies what it can without disturbing what did not change. Focus
//! and bindings are just values, and are swapped. The keyboard is recompiled
//! only if its layout or repeat changed. Pointer settings go to every device.
//! Outputs are relit only if their rules changed, because relighting is a
//! modeset and the screens blink. Autostart is not re-run: those programs are
//! already running.

use std::{path::Path, process::Child};

use perspicax_config::{Built, Config, Pointer};
use smithay::{
    input::keyboard::XkbConfig,
    reexports::input::{Device, DeviceCapability},
};

use super::{Session, relight};
use crate::{Error, Launch, act::Keys, backend::Running, state::Compositor};

/// What this build can honour, for the config's feature check.
const BUILT: Built = Built {
    seat: true,
    xwayland: cfg!(feature = "xwayland"),
};

/// Read the config, or the classic profile without a path.
pub(super) fn load(path: Option<&Path>) -> Result<Config, Error> {
    let Some(path) = path else {
        return Ok(Config::profile(perspicax_config::Profile::Classic, BUILT));
    };
    perspicax_config::load(path, BUILT)
        .map_err(|error| Error::Config(format!("{}: {error}", path.display())))
}

/// Give the seat's keyboard the configured layout and repeat, and rebuild the
/// table agents type from, so the two describe the same keyboard.
pub(super) fn apply_keyboard(state: &mut Compositor) {
    let Running::Seat(session) = &state.backend else {
        return;
    };
    let keyboard_settings = session.settings.keyboard.clone();
    let Some(keyboard) = state.keyboard.clone() else {
        return;
    };
    let xkb = XkbConfig {
        rules: &keyboard_settings.rules,
        model: &keyboard_settings.model,
        layout: &keyboard_settings.layout,
        variant: &keyboard_settings.variant,
        options: keyboard_settings.options.clone(),
    };
    if let Err(error) = keyboard.set_xkb_config(state, xkb) {
        tracing::error!(
            ?error,
            layout = keyboard_settings.layout,
            "xkb could not compile that layout; the keyboard keeps its old one"
        );
        return;
    }
    keyboard.change_repeat_info(
        keyboard_settings.repeat_rate,
        keyboard_settings.repeat_delay,
    );
    state.keys = Keys::from_names(
        &keyboard_settings.rules,
        &keyboard_settings.model,
        &keyboard_settings.layout,
        &keyboard_settings.variant,
        keyboard_settings.options,
    );
}

/// Apply pointer settings to one device, if it is a pointer. Settings left
/// unset keep libinput's own default for that device.
pub(super) fn configure(device: &mut Device, pointer: &Pointer) {
    if !device.has_capability(DeviceCapability::Pointer) {
        return;
    }
    let name = device.name().to_owned();
    let report = |what: &str, result: Result<(), _>| {
        if let Err(error) = result {
            tracing::debug!(
                device = name,
                what,
                ?error,
                "device does not support this setting"
            );
        }
    };
    if let Some(accel) = pointer.accel {
        report("accel", device.config_accel_set_speed(accel));
    }
    if let Some(natural) = pointer.natural_scroll {
        report(
            "natural-scroll",
            device.config_scroll_set_natural_scroll_enabled(natural),
        );
    }
    if let Some(tap) = pointer.tap_to_click
        && device.config_tap_finger_count() > 0
    {
        report("tap-to-click", device.config_tap_set_enabled(tap));
    }
    if let Some(left) = pointer.left_handed {
        report("left-handed", device.config_left_handed_set(left));
    }
}

/// Re-read the config and apply whatever changed. A config that no longer
/// parses is reported and the old one kept: a typo mid-session must not cost
/// the person their bindings.
pub(super) fn reload(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    let fresh = match load(session.config_path.as_deref()) {
        Ok(fresh) => fresh,
        Err(error) => {
            tracing::error!(%error, "config not reloaded; keeping the one in use");
            return;
        }
    };
    let keyboard_changed = fresh.keyboard != session.settings.keyboard;
    let outputs_changed = fresh.outputs != session.settings.outputs;
    session.settings = fresh;
    let pointer = session.settings.pointer;
    for device in &mut session.devices {
        configure(device, &pointer);
    }
    if keyboard_changed {
        apply_keyboard(state);
    }
    if outputs_changed {
        relight(state);
    }
    tracing::info!(keyboard_changed, outputs_changed, "config reloaded");
}

impl Session {
    /// Start the config's autostart programs. A program that will not start
    /// is reported and skipped: one missing panel must not keep the person
    /// out of their session.
    pub(crate) fn autostart(&mut self, launch: Option<&Launch>) {
        let commands = self.settings.autostart.clone();
        for command in &commands {
            self.spawn(launch, command);
        }
    }

    /// Start a program for the person, and keep it to stop with the session.
    pub(super) fn spawn(&mut self, launch: Option<&Launch>, command: &[String]) {
        // Reap what has exited since last time, so a session that starts a
        // terminal a hundred times does not keep a hundred zombies.
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        let Some(launch) = launch else {
            return;
        };
        match launch.spawn(command) {
            Ok(child) => self.children.push(child),
            Err(error) => tracing::warn!(%error, "could not start"),
        }
    }
}

impl Drop for Session {
    /// Stop what the session started. A program left running against a
    /// socket nobody listens on is a process that will never be told to
    /// stop.
    fn drop(&mut self) {
        for child in &mut self.children {
            stop(child);
        }
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
