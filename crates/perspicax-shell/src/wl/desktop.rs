//! The desktop: one surface on the `background` layer of every monitor,
//! with the wallpaper painted on it, and the desktop folder's icons over
//! the first monitor's.
//!
//! Each is anchored to all four edges with an exclusive zone of -1, so it
//! covers the whole monitor and ignores the room a panel reserves. Its
//! namespace is `perspicax-desktop-<connector>`, which is how an agent tells
//! one monitor's desktop from another's and from a window; its accessibility
//! window carries the same name, which is how perspicax joins the two.
//!
//! A changed config repaints the same surfaces, so an agent holding a
//! desktop's id still holds it afterwards. Only turning the wallpaper off
//! takes them away, and turning it on puts them back.
//!
//! The icons go on the first monitor alone, the leftmost, as on Windows, in
//! what the shell's panel leaves of it. The folder is read when they are
//! first shown, and again whenever it or anything in it has changed, which
//! is looked at every two seconds while they are. A click on one redraws the icons alone: the
//! wallpaper under them is painted once and kept.

use std::path::Path;
#[cfg(feature = "icons")]
use std::path::PathBuf;

#[cfg(feature = "icons")]
use accesskit::{Action, ActionHandler, ActionRequest, NodeId};
use perspicax_config::{Shell, Wallpaper};
#[cfg(feature = "icons")]
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use smithay_client_toolkit::{
    output::{OutputInfo, OutputState},
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
use tiny_skia::Pixmap;
#[cfg(feature = "menus")]
use wayland_client::protocol::wl_surface;
use wayland_client::{Proxy, QueueHandle, protocol::wl_output};

#[cfg(feature = "icons")]
use super::Asked;
use super::{
    App,
    canvas::{Canvas, whole},
};
use crate::{
    a11y::{self, adapter::Served},
    model::image,
    paint::{self, Kit},
};
#[cfg(feature = "icons")]
use crate::{
    layout::{self, Rect, folder::Spot},
    model::{
        Button,
        apps::Run,
        desktop::Locale,
        folder::{self, Folder, Pressed},
        fs::Disk,
    },
};

/// What every desktop's namespace starts with, before its monitor's name.
const NAMESPACE: &str = "perspicax-desktop-";

/// Every monitor's desktop, and what they are painted from.
pub(super) struct Desktops {
    /// `None` is no wallpaper, and then no desktop either.
    wallpaper: Option<Wallpaper>,
    /// The wallpaper's image, read once, and again when the config names
    /// another.
    image: Option<Pixmap>,
    each: Vec<Desktop>,
    /// The desktop folder's icons, while they are shown.
    #[cfg(feature = "icons")]
    icons: Option<Icons>,
    /// Where an assistive technology's requests of an icon are sent.
    #[cfg(feature = "icons")]
    actions: Sender<Asked>,
}

/// One monitor's desktop.
struct Desktop {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    /// The monitor's connector name.
    #[cfg_attr(
        not(feature = "menus"),
        expect(
            dead_code,
            reason = "which monitor a click or the icons are on; a wallpaper alone takes neither"
        )
    )]
    name: String,
    /// `perspicax-desktop-<connector>`: the surface's and its window's.
    namespace: String,
    /// Its tree, on the accessibility bus.
    a11y: Served,
    /// The monitor's whole scale: the buffer is this many pixels to each of
    /// the surface's.
    scale: u32,
    /// In the surface's own units, once the compositor has said.
    size: Option<(u32, u32)>,
    /// The wallpaper as last painted under the icons, kept so that showing
    /// a selection does not paint it again.
    #[cfg(feature = "icons")]
    painted: Option<Pixmap>,
    /// Where each icon is, while it holds them.
    #[cfg(feature = "icons")]
    spots: Vec<Spot>,
}

/// The desktop folder, and where its icons are shown.
#[cfg(feature = "icons")]
struct Icons {
    /// `None` with no folder to show.
    dir: Option<PathBuf>,
    /// When the folder or anything in it last changed, as of the last read.
    stamp: Option<(i64, i64)>,
    folder: Folder,
    /// The monitor they are on, by connector name, and the strip of it the
    /// shell's panel takes.
    on: Option<String>,
    strip: Option<Rect>,
    locale: Locale,
    home: Option<PathBuf>,
}

/// A monitor, as the icons are placed by: its connector name, where its
/// top-left corner is on the desk, and the strip of it the shell's panel
/// takes, if it has one.
#[cfg(feature = "icons")]
pub(super) type Monitor = (String, (i32, i32), Option<Rect>);

impl Desktops {
    #[cfg(not(feature = "icons"))]
    pub(super) fn new(shell: &Shell, config: Option<&Path>) -> Self {
        let wallpaper = shell.wallpaper.clone();
        let image = image_of(wallpaper.as_ref(), config);
        Self {
            wallpaper,
            image,
            each: Vec::new(),
        }
    }

    /// The desktops, the folder's icons on them if `shell` asks for them,
    /// what is asked of an icon sent on to `actions`.
    #[cfg(feature = "icons")]
    pub(super) fn new(shell: &Shell, config: Option<&Path>, actions: Sender<Asked>) -> Self {
        let wallpaper = shell.wallpaper.clone();
        let image = image_of(wallpaper.as_ref(), config);
        let mut desktops = Self {
            wallpaper,
            image,
            each: Vec::new(),
            icons: None,
            actions,
        };
        desktops.show_icons(shell);
        desktops
    }

    /// Show the wallpaper `shell` asks for now. Every desktop is painted
    /// again where it is, unless the wallpaper was turned off, which takes
    /// them all away, or on, which puts one on every monitor.
    pub(super) fn reconfigure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        qh: &QueueHandle<App>,
        shell: &Shell,
        config: Option<&Path>,
        outputs: &OutputState,
    ) {
        #[cfg(feature = "icons")]
        let icons_changed = self.show_icons(shell);
        #[cfg(not(feature = "icons"))]
        let icons_changed = false;
        if shell.wallpaper == self.wallpaper {
            if icons_changed {
                self.redraw(canvas, kit);
            }
            return;
        }
        if named(shell.wallpaper.as_ref()) != named(self.wallpaper.as_ref()) {
            self.image = image_of(shell.wallpaper.as_ref(), config);
        }
        let was_on = self.wallpaper.is_some();
        self.wallpaper = shell.wallpaper.clone();
        match (was_on, self.wallpaper.is_some()) {
            (true, true) => {
                #[cfg(feature = "icons")]
                self.each
                    .iter_mut()
                    .for_each(|desktop| desktop.painted = None);
                self.redraw(canvas, kit);
            }
            (true, false) => self.each.clear(),
            (false, true) => {
                for output in outputs.outputs() {
                    if let Some(info) = outputs.info(&output) {
                        self.add(canvas, qh, output, &info);
                    }
                }
            }
            (false, false) => {}
        }
    }

    /// Put a desktop on a monitor the compositor just announced.
    pub(super) fn add(
        &mut self,
        canvas: &Canvas,
        qh: &QueueHandle<App>,
        output: wl_output::WlOutput,
        info: &OutputInfo,
    ) {
        if self.wallpaper.is_none() {
            return;
        }
        let name = info
            .name
            .clone()
            .unwrap_or_else(|| output.id().protocol_id().to_string());
        let namespace = format!("{NAMESPACE}{name}");
        let layer = canvas.layer(qh, Layer::Background, &namespace, &output);
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();
        let tree = a11y::desktop(&namespace, None);
        #[cfg(not(feature = "icons"))]
        let a11y = Served::new(tree);
        #[cfg(feature = "icons")]
        let a11y = Served::acting(tree, Pick(self.actions.clone()));
        self.each.push(Desktop {
            output,
            layer,
            name,
            a11y,
            namespace,
            scale: whole(info.scale_factor),
            size: None,
            #[cfg(feature = "icons")]
            painted: None,
            #[cfg(feature = "icons")]
            spots: Vec::new(),
        });
    }

    /// Paint a monitor's desktop again if its scale changed.
    pub(super) fn rescale(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        output: &wl_output::WlOutput,
        scale: i32,
    ) {
        let scale = whole(scale);
        if let Some(at) = self
            .each
            .iter()
            .position(|desktop| desktop.output == *output && desktop.scale != scale)
        {
            self.each[at].scale = scale;
            self.draw(canvas, kit, at);
        }
    }

    /// Take a monitor's desktop away with it.
    pub(super) fn remove(&mut self, output: &wl_output::WlOutput) {
        self.each.retain(|desktop| desktop.output != *output);
    }

    /// The monitor and size of the desktop that is `surface`, if one is.
    #[cfg(feature = "menus")]
    pub(super) fn at(&self, surface: &wl_surface::WlSurface) -> Option<(&str, (u32, u32))> {
        self.each
            .iter()
            .find(|desktop| desktop.layer.wl_surface() == surface)
            .and_then(|desktop| Some((desktop.name.as_str(), desktop.size?)))
    }

    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        self.each.retain(|desktop| desktop.layer != *layer);
    }

    pub(super) fn configure(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        layer: &LayerSurface,
        configure: &LayerSurfaceConfigure,
    ) {
        let Some(at) = self.each.iter().position(|desktop| desktop.layer == *layer) else {
            return;
        };
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        self.each[at].size = Some((width, height));
        self.draw(canvas, kit, at);
    }

    /// Paint every desktop again.
    fn redraw(&mut self, canvas: &mut Canvas, kit: &mut Kit) {
        (0..self.each.len()).for_each(|at| self.draw(canvas, kit, at));
    }

    /// Paint desktop `at` at its monitor's scale: the wallpaper, and the
    /// icons if it holds them. Its tree on the bus says what it shows.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit, at: usize) {
        let desktop = &mut self.each[at];
        let (Some(wallpaper), Some((width, height))) = (&self.wallpaper, desktop.size) else {
            return;
        };
        // Opaque, which lets the compositor skip whatever is under it.
        let whole = (0, 0, width as i32, height as i32);
        #[cfg(feature = "icons")]
        if let Some(icons) = self
            .icons
            .as_ref()
            .filter(|icons| icons.on.as_ref() == Some(&desktop.name))
        {
            let Kit { fonts, images } = kit;
            let text = fonts.get();
            let monitor = Rect::new(0, 0, width as i32, height as i32);
            let area = layout::usable(monitor, icons.strip);
            let names = icons.folder.icons().iter().map(|icon| icon.name.as_str());
            desktop.spots = layout::folder::lay_out(names, area, &mut *text);
            let pixels = (width * desktop.scale, height * desktop.scale);
            let kept = desktop.painted.take();
            let Some(painted) = painted(kept, pixels, wallpaper, self.image.as_ref()) else {
                return;
            };
            let scale = desktop.scale;
            canvas.show(
                &desktop.layer,
                (width, height),
                scale,
                &[whole],
                |picture| {
                    picture.data_mut().copy_from_slice(painted.data());
                    paint::folder::paint(
                        &icons.folder,
                        &desktop.spots,
                        picture,
                        scale,
                        text,
                        images,
                    );
                },
            );
            desktop.painted = Some(painted);
            desktop.a11y.show(a11y::folder(
                &desktop.namespace,
                desktop.size,
                &icons.folder,
                &desktop.spots,
            ));
            return;
        }
        #[cfg(feature = "icons")]
        {
            desktop.painted = None;
            desktop.spots.clear();
        }
        #[cfg(not(feature = "icons"))]
        let _ = kit;
        canvas.show(
            &desktop.layer,
            (width, height),
            desktop.scale,
            &[whole],
            |picture| {
                paint::wallpaper::paint(wallpaper, self.image.as_ref(), picture);
            },
        );
        desktop
            .a11y
            .show(a11y::desktop(&desktop.namespace, desktop.size));
    }
}

#[cfg(feature = "icons")]
impl Desktops {
    /// Show the desktop folder's icons if `shell` asks for them, and there
    /// is a wallpaper to show them on. Whether what is shown changed.
    fn show_icons(&mut self, shell: &Shell) -> bool {
        let wanted = shell.desktop_icons && shell.wallpaper.is_some();
        match (&mut self.icons, wanted) {
            (None, false) => false,
            (Some(_), false) => {
                self.icons = None;
                true
            }
            (None, true) => {
                self.icons = Some(Icons::find());
                true
            }
            // The desktop folder may be another now.
            (Some(icons), true) => icons.find_again(),
        }
    }

    /// Whether the folder's icons are shown.
    pub(super) fn shows_icons(&self) -> bool {
        self.icons.is_some()
    }

    /// Put the icons on the first of `monitors` that has a desktop, clear of
    /// its panel; draw them there, and take them from where they were.
    pub(super) fn arrange(&mut self, canvas: &mut Canvas, kit: &mut Kit, monitors: &[Monitor]) {
        let Some(icons) = &mut self.icons else {
            return;
        };
        let placed: Vec<(&str, (i32, i32))> = monitors
            .iter()
            .filter(|(name, ..)| self.each.iter().any(|desktop| desktop.name == *name))
            .map(|(name, at, _)| (name.as_str(), *at))
            .collect();
        let on = layout::first(&placed).map(str::to_owned);
        let strip = monitors
            .iter()
            .find(|(name, ..)| Some(name) == on.as_ref())
            .and_then(|&(_, _, strip)| strip);
        if (&on, strip) == (&icons.on, icons.strip) {
            return;
        }
        let was = std::mem::replace(&mut icons.on, on.clone());
        icons.strip = strip;
        for at in 0..self.each.len() {
            let name = Some(&self.each[at].name);
            if name == was.as_ref() || name == on.as_ref() {
                self.draw(canvas, kit, at);
            }
        }
    }

    /// Read the desktop folder again if it changed, and show what it holds
    /// now.
    pub(super) fn refresh(&mut self, canvas: &mut Canvas, kit: &mut Kit) {
        if self.icons.as_mut().is_some_and(Icons::refresh) {
            self.draw_icons(canvas, kit);
        }
    }

    /// A button went down at `point` on the desktop that is `surface`, at
    /// `time` by the pointer's clock: on an icon, or off them. What to
    /// start, if it opened one.
    pub(super) fn press(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        surface: &wl_surface::WlSurface,
        point: (f64, f64),
        button: Button,
        time: u32,
    ) -> Option<Run> {
        let icons = self.icons.as_mut()?;
        let desktop = self.each.iter().find(|desktop| {
            desktop.layer.wl_surface() == surface && icons.on.as_ref() == Some(&desktop.name)
        })?;
        let hit = layout::folder::at(&desktop.spots, point);
        match icons.folder.press(hit, button, time) {
            Pressed::Nothing => None,
            Pressed::Selected => {
                self.draw_icons(canvas, kit);
                None
            }
            Pressed::Open(run) => Some(run),
        }
    }

    /// Select the icon that is node `node` of the tree, as an assistive
    /// technology asked.
    pub(super) fn select(&mut self, canvas: &mut Canvas, kit: &mut Kit, node: NodeId) {
        let Some(icons) = &mut self.icons else {
            return;
        };
        let at = a11y::icon_at(&icons.folder, node);
        if at.is_some() && icons.folder.select(at) == Pressed::Selected {
            self.draw_icons(canvas, kit);
        }
    }

    /// What opening the icon that is node `node` of the tree starts.
    pub(super) fn open(&self, node: NodeId) -> Option<Run> {
        let folder = &self.icons.as_ref()?.folder;
        folder.open(a11y::icon_at(folder, node)?)
    }

    /// Draw the desktop that holds the icons again.
    pub(super) fn draw_icons(&mut self, canvas: &mut Canvas, kit: &mut Kit) {
        let on = self.icons.as_ref().and_then(|icons| icons.on.as_ref());
        if let Some(at) = self
            .each
            .iter()
            .position(|desktop| Some(&desktop.name) == on)
        {
            self.draw(canvas, kit, at);
        }
    }
}

#[cfg(feature = "icons")]
impl Icons {
    /// The desktop folder, wherever the environment says it is, and what it
    /// holds; shown on no monitor yet.
    fn find() -> Self {
        let mut icons = Self {
            dir: desktop_dir(),
            stamp: None,
            folder: Folder::default(),
            on: None,
            strip: None,
            locale: Locale::from_env(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        };
        icons.read();
        icons
    }

    /// Look for the desktop folder again, and read it if it is another.
    /// Whether it was.
    fn find_again(&mut self) -> bool {
        let dir = desktop_dir();
        if dir == self.dir {
            return false;
        }
        self.dir = dir;
        self.read();
        true
    }

    /// Read the folder again if it changed since it was last read. Whether
    /// it did.
    fn refresh(&mut self) -> bool {
        if changed(self.dir.as_deref()) == self.stamp {
            return false;
        }
        self.read();
        true
    }

    fn read(&mut self) {
        self.stamp = changed(self.dir.as_deref());
        let icons = self.dir.as_deref().map_or_else(Vec::new, |dir| {
            folder::read(&Disk, dir, &self.locale, self.home.as_deref())
        });
        tracing::debug!(icons = icons.len(), dir = ?self.dir, "the desktop folder was read");
        self.folder.show(icons);
    }
}

/// `wallpaper` painted `pixels` big: `kept`, if it was painted that size,
/// or else painted afresh.
#[cfg(feature = "icons")]
fn painted(
    kept: Option<Pixmap>,
    (wide, high): (u32, u32),
    wallpaper: &Wallpaper,
    image: Option<&Pixmap>,
) -> Option<Pixmap> {
    if let Some(kept) = kept.filter(|kept| (kept.width(), kept.height()) == (wide, high)) {
        return Some(kept);
    }
    let mut fresh = Pixmap::new(wide, high)?;
    paint::wallpaper::paint(wallpaper, image, &mut fresh.as_mut());
    Some(fresh)
}

/// Where the desktop folder is, as `$XDG_CONFIG_HOME/user-dirs.dirs` says.
#[cfg(feature = "icons")]
fn desktop_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let home = var("HOME");
    let config = var("XDG_CONFIG_HOME").or_else(|| Some(home.as_ref()?.join(".config")));
    folder::desktop_dir(&Disk, config.as_deref(), home.as_deref())
}

/// When the folder at `dir`, or anything in it, last changed, in seconds
/// and nanoseconds: what was read from it is current while this stays the
/// same. A file's status-change time moves when it is written to, renamed
/// or made runnable, and the folder's when something comes or goes; each
/// moves to now, so the newest of them moves whatever changed.
#[cfg(feature = "icons")]
fn changed(dir: Option<&Path>) -> Option<(i64, i64)> {
    use std::os::unix::fs::MetadataExt;

    let dir = dir?;
    let at = |meta: std::fs::Metadata| (meta.ctime(), meta.ctime_nsec());
    let folder = at(std::fs::metadata(dir).ok()?);
    let inside = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        // Followed, so an entry linked from elsewhere counts as itself.
        .filter_map(|entry| std::fs::metadata(entry.path()).ok().map(at));
    inside.chain([folder]).max()
}

/// An assistive technology's requests of an icon, sent on to the shell's
/// loop: clicking one opens it, focusing it selects it.
#[cfg(feature = "icons")]
struct Pick(Sender<Asked>);

#[cfg(feature = "icons")]
impl ActionHandler for Pick {
    fn do_action(&mut self, request: ActionRequest) {
        let node = request.target_node;
        if !a11y::is_icon(node) {
            return;
        }
        let asked = match request.action {
            Action::Click => Asked::OpenIcon(node),
            Action::Focus => Asked::SelectIcon(node),
            _ => return,
        };
        if self.0.send(asked).is_err() {
            tracing::debug!("the shell is stopping; the request is dropped");
        }
    }
}

#[cfg(feature = "icons")]
impl App {
    /// Look at the desktop folder every two seconds while its icons are
    /// shown, and show what changed in it.
    pub(super) fn watch_folder(&mut self) {
        use std::time::Duration;

        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};

        const EVERY: Duration = Duration::from_secs(2);
        if self.watching || !self.desktops.shows_icons() {
            return;
        }
        let watching = self
            .handle
            .insert_source(Timer::from_duration(EVERY), |_, (), app| {
                if !app.desktops.shows_icons() {
                    app.watching = false;
                    return TimeoutAction::Drop;
                }
                app.desktops.refresh(&mut app.canvas, &mut app.kit);
                TimeoutAction::ToDuration(EVERY)
            });
        match watching {
            Ok(_) => self.watching = true,
            Err(error) => tracing::warn!("the desktop folder will not be watched: {error}"),
        }
    }
}

/// The image `wallpaper` names, as the config writes it.
fn named(wallpaper: Option<&Wallpaper>) -> Option<&Path> {
    wallpaper?.image.as_deref()
}

/// The image `wallpaper` names, if it names one that can be read.
fn image_of(wallpaper: Option<&Wallpaper>, config: Option<&Path>) -> Option<Pixmap> {
    read(named(wallpaper)?, config)
}

/// Read the image `written` in the config, or say why not and go without.
fn read(written: &Path, config: Option<&Path>) -> Option<Pixmap> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let path = image::locate(written, config, home.as_deref());
    match image::load(&path) {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::warn!(
                "the wallpaper {} could not be read: {error}; showing its colour instead",
                path.display()
            );
            None
        }
    }
}
