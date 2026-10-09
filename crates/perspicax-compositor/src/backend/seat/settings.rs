//! The person's config, applied to the seat: read at start, re-read on the
//! reload binding, and re-read whenever the file is saved.
//!
//! A reload applies what it can without disturbing what did not change. Focus
//! and bindings are just values, and are swapped. The keyboard is recompiled
//! only if `[input.keyboard]` changed, and keeps the layout in use and its
//! locks (see [`crate::keyboard`]). Pointer settings go to every device.
//! Outputs are relit only if their rules changed, because relighting is a
//! modeset and the screens blink. Autostart, the config's and XDG's, is not
//! re-run: those programs are already running. The shell is told when its
//! own table changed, and is started or stopped when `enabled` was turned on
//! or off (see [`supervise`]).
//!
//! Saving is watched with inotify on the file's *directory*, not the file:
//! most editors save by writing a new file and renaming it over the old one,
//! and a watch on the old file's inode would hear nothing after the first
//! save. A save is often several writes in a row, so a reload waits a moment
//! after the last of them.

use std::{
    mem::MaybeUninit,
    path::{Path, PathBuf},
    process::Child,
    time::Duration,
};

use perspicax_config::{
    Built, Config, Pointer,
    autostart::{self, Skip},
};
use smithay::{
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

use super::{Session, relight, supervise};
use crate::{
    Error, Keymap, Launch,
    backend::{Running, cursor::Cursor},
    state::Compositor,
};

/// What this build can honour, for the config's feature check.
const BUILT: Built = Built {
    seat: true,
    xwayland: cfg!(feature = "xwayland"),
    capture: cfg!(feature = "capture"),
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

/// Give the seat's keyboard the configured keymap and repeat. `numlock` sets
/// Num Lock; `None` leaves it as the person has it.
pub(super) fn apply_keyboard(state: &mut Compositor, numlock: Option<bool>) {
    let Running::Seat(session) = &state.backend else {
        return;
    };
    let settings = session.settings.keyboard.clone();
    state.layout_memory.set_switching(settings.switching);
    let keymap = Keymap {
        rules: settings.rules,
        model: settings.model,
        layout: settings.layout,
        variant: settings.variant,
        options: settings.options,
    };
    if let Err(error) = state.set_keymap(&keymap, numlock) {
        tracing::error!(
            ?error,
            layout = keymap.layout,
            "xkb could not compile that layout; the keyboard keeps its old one"
        );
        return;
    }
    if let Some(keyboard) = &state.keyboard {
        keyboard.change_repeat_info(settings.repeat_rate, settings.repeat_delay);
    }
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
    // Num Lock is the file's when the session starts and when a save changes
    // what it says; in between it is the person's.
    let numlock = fresh
        .keyboard
        .numlock
        .filter(|_| fresh.keyboard.numlock != session.settings.keyboard.numlock);
    let outputs_changed = fresh.outputs != session.settings.outputs;
    let decorations_changed = fresh.decorations != session.settings.decorations;
    let opacity_changed = fresh.opacity != session.settings.opacity;
    let theme_changed = fresh.theme != session.settings.theme;
    let appearance_changed = fresh.theme.apps != session.settings.theme.apps;
    let pointer_changed = (&fresh.theme.cursor, fresh.theme.cursor_size)
        != (
            &session.settings.theme.cursor,
            session.settings.theme.cursor_size,
        );
    let access = (fresh.protocols != session.settings.protocols).then(|| fresh.protocols.clone());
    let mouse_changed = fresh.mouse != session.settings.mouse;
    let mouse = fresh.mouse.clone();
    let shell_changed = fresh.shell != session.settings.shell;
    let shell = session.settings.shell.clone();
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
        apply_keyboard(state, numlock);
    }
    if outputs_changed {
        // The file says where the monitors go now, over anything a display
        // tool asked for since.
        if let Running::Seat(session) = &mut state.backend {
            session.runtime = None;
        }
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
    // The titles' font and the snap preview are drawn from the settings as
    // they are; a frame drawn since only has to be drawn again. The pointer
    // is read again from its theme, and programs started from now on are
    // told; those already running keep the pointer they chose.
    if appearance_changed {
        tell_appearance(state);
    }
    if pointer_changed {
        if let Running::Seat(session) = &mut state.backend {
            let theme = &session.settings.theme;
            session.cursor = Cursor::load(theme.cursor.as_deref(), theme.cursor_size);
        }
        look(state);
    }
    if theme_changed {
        state.backend.redraw();
    }
    // Each application's rule and the dimming are read as each frame is
    // drawn, so only the windows' look and their facts follow. A value the
    // keys set is the window's, and stays.
    if opacity_changed {
        state.backend.redraw();
        state.publish_facts();
    }
    let access_changed = access.is_some();
    if let Some(access) = access {
        state.set_access(access);
    }
    state.mouse = mouse;
    // After the rules, so a shell they no longer admit is not counted as
    // listening.
    supervise::reloaded(state, &shell, access_changed);
    tracing::info!(
        keyboard_changed,
        outputs_changed,
        decorations_changed,
        opacity_changed,
        theme_changed,
        access_changed,
        mouse_changed,
        shell_changed,
        "config reloaded"
    );
}

/// Bring up the person's session around the windows: Xwayland if this build
/// has it and the config wants it, then `ready` -- the agent's programs --
/// then the shell, the autostart list and the XDG autostart entries, the
/// agent's first. All wait for Xwayland, so an X11 program in any of them
/// finds `DISPLAY` set.
pub(crate) fn populate(
    state: &mut Compositor,
    #[cfg_attr(
        not(feature = "xwayland"),
        expect(unused_variables, reason = "Xwayland's")
    )]
    event_loop: &LoopHandle<'static, Compositor>,
    ready: impl FnOnce(&mut Compositor) + 'static,
) -> Result<(), Error> {
    look(state);
    let ready = move |state: &mut Compositor| {
        ready(state);
        // A `--spawn` that would not start ends the session: nothing of the
        // person's is worth starting only to stop.
        if state.spawn_failed.is_none() {
            supervise::start(state);
            autostart(state);
        }
    };
    #[cfg(feature = "xwayland")]
    if matches!(&state.backend, Running::Seat(session) if session.settings.xwayland) {
        return crate::xwayland::start(state, event_loop, ready);
    }
    ready(state);
    Ok(())
}

/// Publish what applications are told about the theme, for whoever serves
/// it to them: the settings portal, in the composition root.
pub(crate) fn tell_appearance(state: &mut Compositor) {
    if let Running::Seat(session) = &state.backend {
        let apps = session.settings.theme.apps;
        state.facts.publish_session(|facts| facts.appearance = apps);
    }
}

/// Tell every program started from now on how the session looks: the
/// pointer's theme and size, where the theme names them. Toolkits draw their
/// own pointer from these, and without them would each pick their own.
fn look(state: &mut Compositor) {
    let Running::Seat(session) = &state.backend else {
        return;
    };
    let theme = &session.settings.theme;
    let look: Vec<(String, String)> = theme
        .cursor
        .iter()
        .map(|name| ("XCURSOR_THEME".to_owned(), name.clone()))
        .chain(
            theme
                .cursor_size
                .map(|size| ("XCURSOR_SIZE".to_owned(), size.to_string())),
        )
        .collect();
    if let Some(launch) = state.launch.as_mut() {
        launch.look = look;
    }
}

fn autostart(state: &mut Compositor) {
    let launch = state.launch.clone();
    if let Running::Seat(session) = &mut state.backend {
        session.autostart(launch.as_ref());
    }
}

impl Session {
    /// Start the config's autostart programs, then the XDG autostart
    /// entries unless the config turns them off. A program that will not
    /// start is reported and skipped: one missing panel must not keep the
    /// person out of their session.
    pub(crate) fn autostart(&mut self, launch: Option<&Launch>) {
        let Some(launch) = launch else {
            return;
        };
        let commands = self.settings.autostart.clone();
        for command in &commands {
            self.spawn_in(launch, command, None);
        }
        if self.settings.xdg_autostart {
            self.xdg_autostart(launch);
        }
    }

    /// Start what the system and the person installed to start with every
    /// desktop (see [`autostart`](mod@autostart)). The folders, the
    /// desktop's names and `PATH` are read as the programs started are given
    /// them.
    fn xdg_autostart(&mut self, launch: &Launch) {
        let var = |name: &str| launch.var(name);
        let found = autostart_files(&autostart::folders(var));
        let path = launch.var("PATH");
        let decided = autostart::select(found, &autostart::desktops(var), |program| {
            installed(program, path.as_deref())
        });
        for (entry, start) in decided {
            match start {
                Ok(start) => self.spawn_in(launch, &start.argv, start.dir.as_deref()),
                // Said where the person would look for why it did not start.
                Err(why @ (Skip::Terminal | Skip::Malformed(_))) => {
                    tracing::info!(entry = %entry, %why, "XDG autostart skips an entry");
                }
                Err(why) => tracing::debug!(entry = %entry, %why, "XDG autostart skips an entry"),
            }
        }
    }

    /// Start a program for the person, and keep it to stop with the session.
    pub(crate) fn spawn(&mut self, launch: Option<&Launch>, command: &[String]) {
        if let Some(launch) = launch {
            self.spawn_in(launch, command, None);
        }
    }

    /// [`spawn`](Self::spawn), in `dir` where it names one.
    fn spawn_in(&mut self, launch: &Launch, command: &[String], dir: Option<&Path>) {
        match launch.spawn_in(command, dir) {
            Ok(child) => self.children.push(child),
            Err(error) => tracing::warn!(%error, "could not start"),
        }
    }
}

/// Every `.desktop` file in the autostart `folders`, the most important
/// folder's first and each folder's by name, with its text, or `None` for
/// one that cannot be read. A folder that is not there holds nothing.
fn autostart_files(folders: &[PathBuf]) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    for folder in folders {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|file| file.file_name().into_string().ok())
            .filter(|name| name.ends_with(".desktop"))
            .collect();
        names.sort();
        found.extend(names.into_iter().map(|name| {
            let text = std::fs::read_to_string(folder.join(&name)).ok();
            (name, text)
        }));
    }
    found
}

/// Whether `program` is installed, as `TryExec` asks: a file that may be
/// run, at the path it names, or else in a folder of `path`.
fn installed(program: &str, path: Option<&str>) -> bool {
    if program.contains('/') {
        return supervise::runnable(Path::new(program));
    }
    path.into_iter()
        .flat_map(std::env::split_paths)
        .any(|folder| supervise::runnable(&folder.join(program)))
}

impl Session {
    /// Collect children that have exited, so a session that starts a
    /// terminal a hundred times does not keep a hundred zombies.
    pub(crate) fn reap(&mut self) {
        self.children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        self.reap_shell();
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

pub(super) fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use std::{env, fs, os::unix::fs::PermissionsExt};

    use super::*;

    /// A fresh directory for one test, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                env::temp_dir().join(format!("perspicax-settings-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch directory");
            Self(dir)
        }

        /// A file at `path` in it, holding `text`, with `mode`.
        fn file(&self, path: &str, text: &str, mode: u32) -> PathBuf {
            let file = self.0.join(path);
            fs::create_dir_all(file.parent().expect("a folder")).expect("its folder");
            fs::write(&file, text).expect("a file");
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).expect("its mode");
            file
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn autostart_reads_each_folders_entries_the_most_important_first() {
        let scratch = Scratch::new("autostart");
        scratch.file("home/autostart/teams.desktop", "the person's", 0o644);
        scratch.file("home/autostart/notes.txt", "not an entry", 0o644);
        scratch.file("etc/autostart/teams.desktop", "the system's", 0o644);
        scratch.file("etc/autostart/pipewire.desktop", "pipewire", 0o644);
        fs::create_dir_all(scratch.0.join("etc/autostart/odd.desktop")).expect("a folder");
        let folders = ["home/autostart", "missing/autostart", "etc/autostart"]
            .map(|folder| scratch.0.join(folder));
        let text = |text: &str| Some(text.to_owned());
        assert_eq!(
            autostart_files(&folders),
            [
                ("teams.desktop".to_owned(), text("the person's")),
                ("odd.desktop".to_owned(), None),
                ("pipewire.desktop".to_owned(), text("pipewire")),
                ("teams.desktop".to_owned(), text("the system's")),
            ],
            "a missing folder holds nothing, and what cannot be read is said"
        );
    }

    #[test]
    fn try_exec_finds_a_program_at_its_path_or_on_path() {
        let scratch = Scratch::new("installed");
        let program = scratch.file("bin/pipewire", "", 0o755);
        scratch.file("bin/notes", "", 0o644);
        let path = format!("/nonexistent:{}", scratch.0.join("bin").display());
        assert!(installed("pipewire", Some(&path)));
        assert!(
            installed(&program.to_string_lossy(), None),
            "a path needs no PATH"
        );
        assert!(!installed("notes", Some(&path)), "one that may not be run");
        assert!(!installed("pipewire", Some("/nonexistent")));
        assert!(!installed("pipewire", None));
    }
}
