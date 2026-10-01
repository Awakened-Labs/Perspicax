//! The person's config, applied to the seat: read at start, re-read on the
//! reload binding, and re-read whenever the file is saved.
//!
//! A reload applies what it can without disturbing what did not change. Focus
//! and bindings are just values, and are swapped. The keyboard is recompiled
//! only if its layout or repeat changed. Pointer settings go to every device.
//! Outputs are relit only if their rules changed, because relighting is a
//! modeset and the screens blink. Autostart is not re-run: those programs are
//! already running.
//!
//! Saving is watched with inotify on the file's *directory*, not the file:
//! most editors save by writing a new file and renaming it over the old one,
//! and a watch on the old file's inode would hear nothing after the first
//! save. A save is often several writes in a row, so a reload waits a moment
//! after the last of them.

use std::{mem::MaybeUninit, path::Path, process::Child, time::Duration};

use perspicax_config::{Built, Config, Pointer};
use smithay::{
    input::keyboard::XkbConfig,
    reexports::calloop::{
        Interest, LoopHandle, Mode, PostAction,
        generic::Generic,
        timer::{TimeoutAction, Timer},
    },
    reexports::input::{Device, DeviceCapability},
    reexports::rustix::{
        fs::inotify::{self, CreateFlags, ReadFlags, Reader, WatchFlags},
        io::Errno,
    },
};

use super::{Session, relight};
use crate::{Error, Launch, act::Keys, backend::Running, state::Compositor};

/// What this build can honour, for the config's feature check.
const BUILT: Built = Built {
    seat: true,
    xwayland: cfg!(feature = "xwayland"),
    // Nothing renders on demand yet; W4 slice 6 adds the `capture` feature.
    capture: false,
};

/// How long a save has to be quiet before it is read: long enough for an
/// editor's write-then-rename to finish, short enough to feel immediate.
const SETTLE: Duration = Duration::from_millis(200);

/// Reload whenever the config file is saved. Nothing to watch without a
/// path, or when its directory does not exist yet: the reload binding still
/// works once it does.
pub(super) fn watch(handle: &LoopHandle<'static, Compositor>, path: Option<&Path>) {
    let Some(path) = path else {
        return;
    };
    let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let name = name.to_owned();
    let watched = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC).and_then(|fd| {
        inotify::add_watch(
            &fd,
            directory,
            WatchFlags::CLOSE_WRITE | WatchFlags::MOVED_TO | WatchFlags::CREATE,
        )?;
        Ok(fd)
    });
    let fd = match watched {
        Ok(fd) => fd,
        Err(error) => {
            tracing::info!(
                directory = %directory.display(),
                %error,
                "the config is not watched; the reload binding still re-reads it"
            );
            return;
        }
    };
    let registered = handle.insert_source(
        Generic::new(fd, Interest::READ, Mode::Level),
        move |_, fd, state: &mut Compositor| {
            let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
            let mut reader = Reader::new(fd.as_ref(), &mut buffer);
            let mut saved = false;
            loop {
                match reader.next() {
                    Ok(event) => {
                        saved |= !event.events().contains(ReadFlags::IGNORED)
                            && event
                                .file_name()
                                .is_some_and(|file| file.to_bytes() == name.as_encoded_bytes());
                    }
                    Err(Errno::WOULDBLOCK | Errno::INTR) => break,
                    Err(error) => {
                        tracing::warn!(%error, "reading the config watch failed");
                        break;
                    }
                }
            }
            if saved {
                settle_then_reload(state);
            }
            Ok(PostAction::Continue)
        },
    );
    match registered {
        Ok(_) => tracing::info!(config = %path.display(), "watching the config for saves"),
        Err(error) => tracing::warn!(%error, "could not watch the config"),
    }
}

/// Reload once saves have been quiet for [`SETTLE`]. A save arriving while
/// one is already waiting changes nothing: the wait reads the file as it is
/// at the end.
fn settle_then_reload(state: &mut Compositor) {
    let Running::Seat(session) = &mut state.backend else {
        return;
    };
    if session.reload_pending {
        return;
    }
    session.reload_pending = true;
    let armed = session
        .handle
        .insert_source(Timer::from_duration(SETTLE), |_, (), state| {
            if let Running::Seat(session) = &mut state.backend {
                session.reload_pending = false;
            }
            tracing::info!("the config was saved");
            reload(state);
            TimeoutAction::Drop
        });
    if let Err(error) = armed {
        tracing::warn!(%error, "could not schedule the config reload");
    }
}

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
pub(crate) fn reload(state: &mut Compositor) {
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
    let decorations_changed = fresh.decorations != session.settings.decorations;
    let access = (fresh.protocols != session.settings.protocols).then(|| fresh.protocols.clone());
    let workspaces = fresh.workspaces;
    if fresh.flipping.delay_ms != session.settings.flipping.delay_ms {
        session.dwell = perspicax_policy::EdgeDwell::new(fresh.flipping.delay_ms);
    }
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
    if workspaces != state.workspaces.shape() {
        state.workspaces.reshape(workspaces);
        state.show_what_belongs();
        state.backend.redraw();
        state.publish_facts();
    }
    // Frames are drawn from the settings as they are each time, but a window
    // sized to fill a zone or a monitor was sized for the old frame.
    if decorations_changed {
        state.refit_frames();
    }
    let access_changed = access.is_some();
    if let Some(access) = access {
        state.set_access(access);
    }
    tracing::info!(
        keyboard_changed,
        outputs_changed,
        decorations_changed,
        access_changed,
        "config reloaded"
    );
}

/// Bring up the person's session around the windows: Xwayland if this build
/// has it and the config wants it, then the autostart list -- after Xwayland
/// is ready, so an X11 program in it finds `DISPLAY` set.
pub(crate) fn populate(
    state: &mut Compositor,
    #[cfg_attr(
        not(feature = "xwayland"),
        expect(unused_variables, reason = "Xwayland's")
    )]
    event_loop: &LoopHandle<'static, Compositor>,
) -> Result<(), Error> {
    #[cfg(feature = "xwayland")]
    if matches!(&state.backend, Running::Seat(session) if session.settings.xwayland) {
        return crate::xwayland::start(state, event_loop, autostart);
    }
    autostart(state);
    Ok(())
}

fn autostart(state: &mut Compositor) {
    let launch = state.launch.clone();
    if let Running::Seat(session) = &mut state.backend {
        session.autostart(launch.as_ref());
    }
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
    pub(crate) fn spawn(&mut self, launch: Option<&Launch>, command: &[String]) {
        let Some(launch) = launch else {
            return;
        };
        match launch.spawn(command) {
            Ok(child) => self.children.push(child),
            Err(error) => tracing::warn!(%error, "could not start"),
        }
    }
}

impl Session {
    /// Collect children that have exited, so a session that starts a
    /// terminal a hundred times does not keep a hundred zombies.
    pub(crate) fn reap(&mut self) {
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
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
