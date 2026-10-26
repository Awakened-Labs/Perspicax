//! What is shown on the screen a moment: the volume when it changes, and
//! why something the person asked for was refused. A surface on the
//! `overlay` layer, of no monitor of its own, which perspicax puts on the
//! one the pointer is on, standing [`ABOVE`] the foot of the room beside a
//! panel there. It takes no clicks and never the keyboard. It shows the
//! last to come, sized to it, and goes [`SHOWN_FOR`] after a volume and
//! [`NOTICE_FOR`] after a notice. Its namespace is `perspicax-osd`.
//!
//! It is drawn at scale 1 until the compositor says which monitor it is
//! on, then at that monitor's.

use std::time::Duration;

use smithay_client_toolkit::{
    output::OutputState,
    reexports::calloop::{
        RegistrationToken,
        timer::{TimeoutAction, Timer},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerSurface, LayerSurfaceConfigure},
    },
};
use wayland_client::protocol::{wl_output, wl_surface};

use super::{
    App,
    canvas::{Canvas, whole},
};
use crate::{
    a11y::{self, adapter::Served},
    layout::osd::{ABOVE, Showing},
    model::notice::{Refused, notice},
    paint::{self, Kit},
};

/// How long the volume stays after the last change it shows.
#[cfg(feature = "audio")]
pub(super) const SHOWN_FOR: Duration = Duration::from_millis(1500);
/// How long a notice stays: long enough to read two lines.
pub(super) const NOTICE_FOR: Duration = Duration::from_secs(5);
const NAMESPACE: &str = "perspicax-osd";

/// What is shown, while it is.
#[derive(Default)]
pub(super) struct Osd {
    shown: Option<Shown>,
    /// What it shows, and the timer that takes it away.
    showing: Option<Showing>,
    going: Option<RegistrationToken>,
}

/// Its surface.
struct Shown {
    layer: LayerSurface,
    a11y: Served,
    scale: u32,
    /// The size asked for, in the surface's own units.
    wanted: (u32, u32),
    /// The size given, once the compositor has said.
    size: Option<(u32, u32)>,
}

impl Osd {
    /// Show `showing`, putting the surface up if it is not, and sizing it
    /// again if it is shown at another size.
    fn show(
        &mut self,
        canvas: &mut Canvas,
        qh: &wayland_client::QueueHandle<App>,
        kit: &mut Kit,
        showing: Showing,
    ) {
        let wanted = showing.size(&mut kit.fonts);
        self.showing = Some(showing);
        match &mut self.shown {
            Some(shown) if shown.wanted != wanted => {
                // Drawn again once the compositor has given the new size.
                shown.layer.set_size(wanted.0, wanted.1);
                shown.layer.commit();
                shown.wanted = wanted;
                shown.size = None;
            }
            Some(_) => {}
            None => {
                let layer = canvas.layer(qh, Layer::Overlay, NAMESPACE, None);
                layer.set_anchor(Anchor::BOTTOM);
                layer.set_size(wanted.0, wanted.1);
                layer.set_margin(0, 0, ABOVE, 0);
                layer.set_exclusive_zone(0);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                canvas.take_no_clicks(&layer);
                layer.commit();
                self.shown = Some(Shown {
                    layer,
                    a11y: Served::new(a11y::osd(NAMESPACE, None, None)),
                    scale: 1,
                    wanted,
                    size: None,
                });
            }
        }
        self.draw(canvas, kit);
    }

    /// Take it away.
    fn hide(&mut self) {
        self.shown = None;
        self.showing = None;
        self.going = None;
    }

    /// Draw it, once the compositor has given it a size.
    fn draw(&mut self, canvas: &mut Canvas, kit: &mut Kit) {
        let (Some(shown), Some(showing)) = (&mut self.shown, &self.showing) else {
            return;
        };
        let Some(size) = shown.size else {
            return;
        };
        let Kit { fonts, palette, .. } = kit;
        let text = fonts.get();
        let scale = shown.scale;
        let (wide, high) = (size.0 as i32, size.1 as i32);
        canvas.show(
            &shown.layer,
            size,
            scale,
            &[(0, 0, wide, high)],
            |picture| {
                paint::osd::paint(showing, picture, size, scale, text, palette);
            },
        );
        shown
            .a11y
            .show(a11y::osd(NAMESPACE, Some(size), Some(showing)));
    }

    /// The compositor sized the surface: draw it.
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
        self.draw(canvas, kit);
    }

    /// The compositor took the surface away.
    pub(super) fn closed(&mut self, layer: &LayerSurface) {
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.layer == *layer)
        {
            self.shown = None;
        }
    }

    /// `surface` came onto `output`: if it is this one's, draw it at that
    /// monitor's scale.
    pub(super) fn entered(
        &mut self,
        canvas: &mut Canvas,
        kit: &mut Kit,
        outputs: &OutputState,
        (surface, output): (&wl_surface::WlSurface, &wl_output::WlOutput),
    ) {
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.layer.wl_surface() == surface)
        else {
            return;
        };
        let scale = outputs
            .info(output)
            .map_or(1, |info| whole(info.scale_factor));
        if scale != shown.scale {
            shown.scale = scale;
            self.draw(canvas, kit);
        }
    }
}

impl App {
    /// Show `showing` in place of whatever is shown, for [`SHOWN_FOR`] from
    /// now if it is the volume, or [`NOTICE_FOR`] if a notice.
    pub(super) fn indicate(&mut self, showing: Showing) {
        let lasting = match showing {
            #[cfg(feature = "audio")]
            Showing::Volume(_) => SHOWN_FOR,
            Showing::Notice(_) => NOTICE_FOR,
        };
        let qh = self.qh.clone();
        self.osd.show(&mut self.canvas, &qh, &mut self.kit, showing);
        if let Some(going) = self.osd.going.take() {
            self.handle.remove(going);
        }
        let going = self
            .handle
            .insert_source(Timer::from_duration(lasting), |_, (), app| {
                app.osd.hide();
                TimeoutAction::Drop
            });
        match going {
            Ok(going) => self.osd.going = Some(going),
            Err(error) => {
                tracing::warn!("what is shown will not go: {error}");
                self.osd.hide();
            }
        }
    }

    /// Something the person asked for was refused: say why.
    pub(super) fn refused(&mut self, refused: &Refused) {
        let notice = notice(refused);
        tracing::info!(
            headline = notice.headline,
            why = notice.why,
            "saying why something was refused"
        );
        self.indicate(Showing::Notice(notice));
    }
}
