//! How see-through a window is drawn: the person's `[opacity]`, carried out.
//!
//! For the person's eyes alone. Nothing here reaches what an agent is told a
//! window covers, which is judged from what its client declared and from its
//! frame (`perspicax_index::host`); a picture an agent takes shows what the
//! person sees.
//!
//! What a window is drawn at is decided afresh for every frame and every
//! picture, from what the window is now: a key-set value is the only thing
//! remembered, in its [`Placement`](super::Placement), and a rule changed by a
//! reload applies to the next frame.

use perspicax_node::SurfaceId;
use perspicax_policy::OPAQUE;

use super::{Fill, id_of, placement};
use crate::{framed::Framed, state::Compositor};

impl Compositor {
    /// What `window` is drawn at now, in percent. `in_use` is the window the
    /// keys go to ([`Compositor::window_in_use`]), asked once by a caller
    /// drawing many.
    ///
    /// Wholly opaque, whatever is asked, for:
    /// - An X11 window that bypasses the window manager: a menu or a tooltip.
    ///   It never has the keyboard, so dimming what lacks it would dim every
    ///   one, and nothing ties it to the window it belongs to.
    /// - A fullscreen window, in use or not: a video is not dimmed by a rule
    ///   meant for its window. Its own value returns when it leaves.
    ///
    /// Any other is drawn at its own, the keys' or its application's, dimmed
    /// while it is not in use.
    pub(crate) fn opacity_of(&self, window: &Framed, in_use: Option<SurfaceId>) -> u8 {
        #[cfg(feature = "xwayland")]
        if window
            .x11_surface()
            .is_some_and(smithay::xwayland::X11Surface::is_override_redirect)
        {
            return OPAQUE;
        }
        if Self::filling(window) == Some(Fill::Fullscreen) {
            return OPAQUE;
        }
        let in_use = in_use.is_some() && id_of(window) == in_use;
        self.backend
            .opacity()
            .shown(self.own_opacity(window), in_use)
    }

    /// What `window` has of its own: what the keys set, or else its
    /// application's rule, or else opaque.
    fn own_opacity(&self, window: &Framed) -> u8 {
        let opacity = self.backend.opacity();
        placement(window, |placement| placement.opacity).unwrap_or_else(|| {
            // An app id is read under a lock and copied: not for nothing.
            if opacity.apps.is_empty() {
                OPAQUE
            } else {
                opacity.ruled(crate::toplevels::app_id(window).as_deref())
            }
        })
    }
}

/// A percent as the alpha a renderer takes.
#[cfg(any(feature = "seat", feature = "capture"))]
pub(crate) fn alpha(percent: u8) -> f32 {
    f32::from(percent) / f32::from(OPAQUE)
}
