//! The menus' surface: while a menu is open, one surface on the `overlay`
//! layer of its monitor, over everything, a fullscreen window included.
//!
//! It covers the whole monitor and is clear but for the menus, so a click
//! anywhere off them lands on it and closes them, as a click away from a
//! menu does on any desktop. It takes the keyboard while it is up, and says
//! it is opaque only where a menu is, so an agent knows what it covers. Its
//! namespace is `perspicax-menu-<connector>`, and its accessibility window
//! carries the same name, which is how perspicax joins the two.
//!
//! It covers the whole monitor, panels included, rather than only the room
//! panels leave: a point perspicax gives, or a click on the wallpaper, is
//! from the monitor's corner, and a surface placed in what panels leave
//! cannot know where that corner is. The shell's own panel is cut out of
//! where it takes clicks, and no menu is drawn over it, so the panel stays
//! in reach while a menu is open; another program's panel is covered, and a
//! click on it closes the menus (issue #24 would change that).
//!
//! The menus are built from the installed applications when one first
//! opens, and again on an open after an application was installed or
//! removed, or the menu file changed: what they list is never older than
//! the last time one was opened.

use std::{
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use accesskit::{Action, ActionHandler, ActionRequest};
use perspicax_config::Shell;
use smithay_client_toolkit::{
    output::OutputState,
    reexports::calloop::channel::Sender,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::{QueueHandle, protocol::wl_output, protocol::wl_surface};

use super::{
    App, Asked,
    canvas::{Area, Canvas, whole},
    installed::Installed,
};
use crate::{
    a11y::{self, adapter::Served},
    launch::{self, Launcher},
    layout::{Rect, usable},
    model::{
        Button,
        apps::{Places, Run},
        fs::{Disk, which},
        image,
        menu::{self, Session},
        menu_file,
    },
    paint::{self, Kit, text::Fonts},
    update::{Effect, Event, State},
};

/// The menus, and everything they are built and drawn from.
pub(super) struct Menus {
    state: State,
    /// The surface, while a menu is open.
    shown: Option<Shown>,
    /// The strip each of the shell's panels takes of its monitor, by the
    /// monitor's connector name: where no menu is drawn, and where the
    /// menus' surface takes no clicks.
    reserved: Vec<(String, Rect)>,
    settings: Settings,
    /// The folders programs are found in: Lock's, and a terminal.
    path: Vec<PathBuf>,
    /// What the menus were last built from: the read of the applications,
    /// and when the menu file last changed. `None` to build them afresh.
    built_from: Option<(u64, Option<SystemTime>)>,
    /// Whether there is a compositor to ask to end the session.
    log_out: bool,
    pub(super) launcher: Launcher,
    /// Where an assistive technology's requests are sent, to reach the loop.
    actions: Sender<Asked>,
}

/// What the config says of the menus.
#[derive(Debug, Clone, PartialEq)]
struct Settings {
    root: bool,
    menu_file: Option<PathBuf>,
    lock: Vec<String>,
}

/// The menus' surface.
struct Shown {
    /// The monitor's connector name.
    name: String,
    namespace: String,
    layer: LayerSurface,
    a11y: Served,
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
}

impl Menus {
    pub(super) fn new(
        shell: &Shell,
        config: Option<&Path>,
        places: &Places,
        actions: Sender<Asked>,
        log_out: bool,
    ) -> Self {
        let terminal = shell
            .terminal
            .clone()
            .or_else(|| launch::find_terminal(&Disk, &places.path));
        Self {
            state: State::new(menu::Menu::default(), menu::Menu::default()),
            shown: None,
            reserved: Vec::new(),
            settings: Settings::of(shell, config),
            path: places.path.clone(),
            built_from: None,
            log_out,
            launcher: Launcher::new(terminal, home()),
            actions,
        }
    }

    /// Take up a changed config. The menus are built afresh when one next
    /// opens.
    pub(super) fn reconfigure(&mut self, shell: &Shell, config: Option<&Path>) {
        self.settings = Settings::of(shell, config);
        self.launcher.set_terminal(
            shell
                .terminal
                .clone()
                .or_else(|| launch::find_terminal(&Disk, &self.path)),
        );
        self.built_from = None;
    }

    /// Whether Log Out has a compositor to ask.
    pub(super) fn set_log_out(&mut self, log_out: bool) {
        if log_out != self.log_out {
            self.log_out = log_out;
            self.built_from = None;
        }
    }

    /// Whether `surface` is the menus'.
    pub(super) fn owns(&self, surface: &wl_surface::WlSurface) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|shown| shown.layer.wl_surface() == surface)
    }

    /// Where the shell's panels are, from now on.
    #[cfg(feature = "panel")]
    pub(super) fn reserve(&mut self, reserved: Vec<(String, Rect)>) {
        self.reserved = reserved;
    }

    /// The strip the shell's panel takes of the monitor named `output`.
    pub(super) fn reserved_on(&self, output: &str) -> Option<Rect> {
        reserved_on(&self.reserved, output)
    }

    /// The monitor the start menu is open on, if it is open.
    #[cfg(feature = "panel")]
    pub(super) fn start_open_on(&self) -> Option<&str> {
        self.state.start_open_on()
    }

    /// Handle `event`, and show what it changed. What is left to do, the
    /// effects that reach past the menus, is returned.
    pub(super) fn send(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        outputs: &OutputState,
        kit: &mut Kit,
        installed: &mut Installed,
        event: Event,
    ) -> Vec<Effect> {
        let root = matches!(
            event,
            Event::RootMenu { .. }
                | Event::DesktopPress {
                    button: Button::Right,
                    ..
                }
        );
        if root && !self.settings.root {
            return Vec::new();
        }
        if (root || matches!(event, Event::StartMenu { .. })) && !self.state.is_open() {
            self.refresh(installed);
        }
        let effects = self.state.update(event, &mut kit.fonts);
        if effects.contains(&Effect::Redraw) {
            self.sync(canvas, qh, outputs, kit);
        }
        effects
            .into_iter()
            .filter(|effect| *effect != Effect::Redraw)
            .collect()
    }

    /// Build the menus again if what they are built from changed.
    fn refresh(&mut self, installed: &mut Installed) {
        let menu_file = self.settings.menu_file.as_deref();
        let now = (installed.refresh(), modified(menu_file));
        if self.built_from == Some(now) {
            return;
        }
        let apps = installed.apps();
        let file = menu_file.and_then(|path| match menu_file::read(path, home().as_deref()) {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(
                    "the menu file {} cannot be used: {error}; showing the applications",
                    path.display()
                );
                None
            }
        });
        let lock = self
            .settings
            .lock
            .first()
            .filter(|program| which(&Disk, program, &self.path).is_some())
            .map(|_| Run {
                argv: self.settings.lock.clone(),
                terminal: false,
                dir: None,
            });
        let session = Session {
            lock,
            log_out: self.log_out,
        };
        self.state.set_menus(
            menu::root(apps, file.as_ref(), &session),
            menu::start(apps, &session),
        );
        tracing::debug!(applications = apps.len(), "the menus were built");
        self.built_from = Some(now);
    }

    /// Make the surface match what is open: take it away, put it up on the
    /// monitor a menu opened on, or draw it again.
    fn sync(
        &mut self,
        canvas: &mut Canvas,
        qh: &QueueHandle<App>,
        outputs: &OutputState,
        kit: &mut Kit,
    ) {
        let Some(view) = self.state.view() else {
            self.shown = None;
            return;
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.name != view.output)
        {
            self.shown = None;
        }
        if self.shown.is_none() {
            let found = outputs.outputs().find_map(|output| {
                let info = outputs.info(&output)?;
                (info.name.as_deref() == Some(view.output)).then_some((output, info.scale_factor))
            });
            let Some((output, scale)) = found else {
                tracing::warn!(
                    output = view.output,
                    "a menu was opened on a monitor that is gone"
                );
                return;
            };
            let namespace = format!("perspicax-menu-{}", view.output);
            self.shown = Some(Shown::put_up(
                canvas,
                qh,
                &output,
                view.output,
                namespace,
                whole(scale),
                Forward(self.actions.clone()),
            ));
        }
        self.draw(canvas, kit);
    }

    /// Draw the open menus on the surface, once the compositor has given it
    /// a size.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit) {
        let (Some(shown), Some(view)) = (&mut self.shown, self.state.view()) else {
            return;
        };
        let Some(size) = shown.size else {
            return;
        };
        let panel = reserved_on(&self.reserved, &shown.name)
            .map(|strip| (strip.x, strip.y, strip.w, strip.h));
        canvas.take_clicks(&shown.layer, size, panel);
        let opaque: Vec<Area> = view
            .menus
            .iter()
            .map(|menu| (menu.rect.x, menu.rect.y, menu.rect.w, menu.rect.h))
            .collect();
        let Kit { fonts, images } = kit;
        let text = fonts.get();
        let scale = shown.scale;
        let started = Instant::now();
        canvas.show(&shown.layer, size, scale, &opaque, |picture| {
            paint::menu::paint(&view, picture, scale, text, images);
        });
        tracing::debug!(took = ?started.elapsed(), "the menus were drawn");
        shown
            .a11y
            .show(a11y::menu(&shown.namespace, Some(size), Some(&view)));
    }

    /// The compositor sized the surface: lay the menus out on it and draw
    /// them.
    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        layer: &LayerSurface,
        configure: &LayerSurfaceConfigure,
    ) {
        let Some(shown) = self.shown.as_mut().filter(|shown| shown.layer == *layer) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        shown.size = Some((width, height));
        let monitor = Rect::new(0, 0, width as i32, height as i32);
        let area = usable(monitor, reserved_on(&self.reserved, &shown.name));
        self.state.update(Event::Resized(area), &mut kit.fonts);
        self.draw(canvas, kit);
    }

    /// The compositor took the surface away: the monitor went.
    pub(super) fn closed(&mut self, fonts: &mut Fonts, layer: &LayerSurface) {
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.layer == *layer)
        {
            self.shown = None;
            self.state.update(Event::KeyboardLost, fonts);
        }
    }

    /// The keyboard came to `surface`, or left it.
    pub(super) fn keyboard(&mut self, surface: &wl_surface::WlSurface, entered: bool) -> bool {
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.layer.wl_surface() == surface)
        else {
            return false;
        };
        shown.a11y.focused(entered);
        true
    }
}

impl Shown {
    /// A clear surface over the whole of `output` that takes the keyboard.
    fn put_up(
        canvas: &Canvas,
        qh: &QueueHandle<App>,
        output: &wl_output::WlOutput,
        name: &str,
        namespace: String,
        scale: u32,
        actions: Forward,
    ) -> Self {
        let layer = canvas.layer(qh, Layer::Overlay, &namespace, output);
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        Self {
            name: name.to_owned(),
            a11y: Served::acting(a11y::menu(&namespace, None, None), actions),
            namespace,
            layer,
            scale,
            size: None,
        }
    }
}

impl Settings {
    fn of(shell: &Shell, config: Option<&Path>) -> Self {
        let home = home();
        Self {
            root: shell.root_menu,
            menu_file: shell
                .menu_file
                .as_deref()
                .map(|written| image::locate(written, config, home.as_deref())),
            lock: shell.lock.clone(),
        }
    }
}

/// The strip of `reserved` on the monitor named `output`.
fn reserved_on(reserved: &[(String, Rect)], output: &str) -> Option<Rect> {
    reserved
        .iter()
        .find(|(name, _)| name == output)
        .map(|&(_, strip)| strip)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// When the menu file last changed, if there is one: a menu built from it
/// is current while this stays the same.
fn modified(menu_file: Option<&Path>) -> Option<SystemTime> {
    std::fs::metadata(menu_file?)
        .and_then(|meta| meta.modified())
        .ok()
}

/// An assistive technology's requests of a menu, sent on to the shell's
/// loop: clicking an item chooses it, focusing it selects it.
struct Forward(Sender<Asked>);

impl ActionHandler for Forward {
    fn do_action(&mut self, request: ActionRequest) {
        let Some(route) = a11y::route_of(request.target_node) else {
            return;
        };
        let event = match request.action {
            Action::Click => Event::Choose(route),
            Action::Focus => Event::Select(route),
            _ => return,
        };
        if self.0.send(Asked::Menu(event)).is_err() {
            tracing::debug!("the shell is stopping; the request is dropped");
        }
    }
}
