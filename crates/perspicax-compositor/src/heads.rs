//! The monitors, as a display tool sees them, and changing them at its
//! request.
//!
//! Whether a request can be carried out is `perspicax_policy::check_heads`'s
//! to say; this is carrying it out, per backend. Headless, a monitor is a
//! virtual one, and any size can be asked of it. On a seat, the request
//! becomes the output rules in force for the rest of the session, exactly as
//! if the config file had said it, and the monitors are lit again from them
//! by the same code that lit them at login. The file itself is not touched:
//! it stays what the person wrote, and a reload that changes `[[output]]`
//! puts it back in force.

use perspicax_policy::{Head, HeadChange, HeadMode, ModeChoice, Place};
use smithay::output::{Mode, Scale};

use crate::{
    backend::{Plugged, Running, Virtual},
    state::Compositor,
};

/// A virtual monitor's refresh, in millihertz.
const VIRTUAL_REFRESH: i32 = 60_000;

impl Compositor {
    /// Every monitor, on or off.
    pub(crate) fn heads(&self) -> Vec<Head> {
        match &self.backend {
            Running::Headless { outputs, dark, .. } => outputs
                .iter()
                .map(|plugged| {
                    let output = &plugged.output;
                    let mode = output.current_mode();
                    Head {
                        name: output.name(),
                        enabled: true,
                        modes: mode
                            .map(|mode| HeadMode {
                                width: mode.size.w,
                                height: mode.size.h,
                                refresh: mode.refresh,
                                preferred: true,
                            })
                            .into_iter()
                            .collect(),
                        current: mode.map(|_| 0),
                        position: self
                            .space
                            .output_geometry(output)
                            .map_or((0, 0), |area| (area.loc.x, area.loc.y)),
                        scale: output.current_scale().fractional_scale(),
                    }
                })
                .chain(dark.iter().map(|virtual_output| Head {
                    name: virtual_output.name.clone(),
                    enabled: false,
                    modes: vec![HeadMode {
                        width: virtual_output.size.0,
                        height: virtual_output.size.1,
                        refresh: VIRTUAL_REFRESH,
                        preferred: true,
                    }],
                    current: None,
                    position: (0, 0),
                    scale: 1.0,
                }))
                .collect(),
            #[cfg(feature = "seat")]
            Running::Seat(session) => session.heads(&self.space),
        }
    }

    /// Whether a monitor may be given a mode it does not list: a virtual one
    /// may be any size.
    pub(crate) fn custom_modes(&self) -> bool {
        matches!(self.backend, Running::Headless { .. })
    }

    /// Carry out a request `check_heads` accepted.
    pub(crate) fn apply_heads(&mut self, changes: &[HeadChange]) {
        match &mut self.backend {
            Running::Headless { .. } => self.apply_virtual(changes),
            #[cfg(feature = "seat")]
            Running::Seat(_) => crate::backend::seat::apply_heads(self, changes),
        }
        self.publish_facts();
    }

    fn apply_virtual(&mut self, changes: &[HeadChange]) {
        for change in changes {
            let (outputs, dark) = match &mut self.backend {
                Running::Headless { outputs, dark, .. } => (outputs, dark),
                #[cfg(feature = "seat")]
                Running::Seat(_) => return,
            };
            let lit = outputs
                .iter()
                .position(|plugged| plugged.output.name() == change.name);
            match (lit, change.enabled) {
                (Some(at), false) => {
                    let gone = outputs.remove(at);
                    let size = gone
                        .output
                        .current_mode()
                        .map_or((0, 0), |mode| (mode.size.w, mode.size.h));
                    dark.push(Virtual {
                        name: change.name.clone(),
                        size,
                        place: gone.place.clone(),
                    });
                    self.workspaces.forget_output(&change.name);
                    crate::layers::close_on(&gone.output);
                    self.space.unmap_output(&gone.output);
                    self.display.remove_global::<Compositor>(gone.global);
                }
                (None, true) => {
                    let Some(at) = dark.iter().position(|dark| dark.name == change.name) else {
                        continue;
                    };
                    let mut virtual_output = dark.remove(at);
                    reshape(&mut virtual_output, change);
                    let plugged = crate::backend::plug(&self.display, &virtual_output);
                    rescale(&plugged, change);
                    outputs.push(plugged);
                }
                (Some(at), true) => {
                    let plugged = &mut outputs[at];
                    if let Some(ModeChoice::Custom { width, height, .. }) = change.mode {
                        let mode = Mode {
                            size: (width, height).into(),
                            refresh: VIRTUAL_REFRESH,
                        };
                        plugged
                            .output
                            .change_current_state(Some(mode), None, None, None);
                        plugged.output.set_preferred(mode);
                    }
                    if let Some((x, y)) = change.position {
                        plugged.place = Place::At(x, y);
                    }
                    rescale(plugged, change);
                }
                (None, false) => {}
            }
        }
        self.arrange_outputs();
    }
}

/// A virtual monitor coming back on, at the size and place asked for.
fn reshape(virtual_output: &mut Virtual, change: &HeadChange) {
    if let Some(ModeChoice::Custom { width, height, .. }) = change.mode {
        virtual_output.size = (width, height);
    }
    if let Some((x, y)) = change.position {
        virtual_output.place = Place::At(x, y);
    }
}

fn rescale(plugged: &Plugged, change: &HeadChange) {
    if let Some(scale) = change.scale {
        let scale = if scale.fract() == 0.0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a whole number from 1 to 4, checked by check_heads"
            )]
            Scale::Integer(scale as i32)
        } else {
            Scale::Fractional(scale)
        };
        plugged
            .output
            .change_current_state(None, None, Some(scale), None);
    }
}
