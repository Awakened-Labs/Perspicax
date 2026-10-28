//! A token for what the shell starts (#258).
//!
//! A program the person picks in a menu, a pie or the desktop folder is
//! handed an activation token, as `XDG_ACTIVATION_TOKEN` and
//! `DESKTOP_STARTUP_ID`, so that its window opens with the keys, the person
//! having asked for it, as perspicax opens one only when they did (#259);
//! and so that a program that hands on to a copy of itself already running
//! brings that copy's window forward. The token is asked for with the
//! serial of the person's latest press on the shell, or of the keyboard
//! coming to it, while the shell still has the keys: the menu or pie picked
//! from is let go only after what was picked has asked. The program starts
//! once the compositor answers, which it always does, with a token it will
//! not grant included. With no `xdg_activation_v1`, or nothing pressed on
//! the shell yet, it starts at once, handed none.

use smithay_client_toolkit::{
    activation::{ActivationHandler, RequestDataExt},
    delegate_activation,
};
use wayland_client::protocol::{wl_seat, wl_surface};

use super::App;
use crate::model::apps::Run;

/// A program picked, until the token it is to be handed comes.
pub(crate) struct Starting {
    pub(super) run: Run,
    pub(super) seat_and_serial: (wl_seat::WlSeat, u32),
    /// The surface pressed, while it is there.
    pub(super) surface: Option<wl_surface::WlSurface>,
}

impl RequestDataExt for Starting {
    fn app_id(&self) -> Option<&str> {
        None
    }

    fn seat_and_serial(&self) -> Option<(&wl_seat::WlSeat, u32)> {
        let (seat, serial) = &self.seat_and_serial;
        Some((seat, *serial))
    }

    fn surface(&self) -> Option<&wl_surface::WlSurface> {
        self.surface.as_ref()
    }
}

impl ActivationHandler for App {
    type RequestData = Starting;

    fn new_token(&mut self, token: String, starting: &Starting) {
        self.launch(&starting.run, Some(&token));
    }
}

delegate_activation!(App, Starting);
