//! A window of one program that another program's dialog is for:
//! xdg-foreign, v2 (#235).
//!
//! A program asks another to show a dialog for one of its windows: Firefox
//! asks the desktop portal for a file chooser, a sandboxed program asks it
//! for a screenshot. The dialog is the portal's window, but its parent is
//! the program's, and xdg-shell names a parent only among one program's own
//! windows. So the program exports its window, `zxdg_exporter_v2`, and is
//! handed a string it passes on, over D-Bus, as the portal's
//! `parent_window`; the portal imports it, `zxdg_importer_v2`, and makes the
//! window it names its dialog's parent.
//!
//! Smithay makes that window the dialog's xdg parent, as if the dialog's own
//! program had named it, and says so through `parent_changed`. Everything
//! that asks what a window hangs from is told it, with no code of its own:
//! the dialog is kept on top with its window and let go with it
//! (`shell::stacking`), it is no main window to be remembered under the
//! portal's name (`shell::remember`), and a taskbar is told whose it is
//! (`toplevels`). When the program revokes its export, or the portal lets its
//! import go, smithay takes the parent away again, and says so the same way.
//!
//! Nobody is refused the protocol: a program exports only its own windows,
//! and imports only a handle it was given. `[protocols]` keeps the ones that
//! reach past a program's own windows.
//!
//! Only v2. GTK 3 and 4, Qt 6 and Chromium speak it; smithay has no v1.
//!
//! # A handle that names no window
//!
//! xdg-foreign exports only a toplevel, and refuses anything else as it is
//! exported. Smithay 0.7 exports any surface, and finds out only when that
//! surface is named as a parent: it then refuses the dialog, ending the
//! *importer*, the portal, with `invalid_surface`. A program chooses the
//! handle it hands the portal, so any program could end the portal for
//! everyone. So a surface that is not a toplevel is refused here, as the
//! protocol says, and it is the program exporting it that ends.

use smithay::{
    reexports::{
        wayland_protocols::xdg::foreign::zv2::server::{
            zxdg_exported_v2::ZxdgExportedV2,
            zxdg_exporter_v2::{self, ZxdgExporterV2},
            zxdg_imported_v2::ZxdgImportedV2,
            zxdg_importer_v2::ZxdgImporterV2,
        },
        wayland_server::{Resource as _, delegate_dispatch, delegate_global_dispatch},
    },
    wayland::{
        compositor,
        shell::xdg::XDG_TOPLEVEL_ROLE,
        xdg_foreign::{
            XdgExportedUserData, XdgForeignHandler, XdgForeignState, XdgImportedUserData,
        },
    },
};

use crate::{sizes::checked, state::Compositor};

impl XdgForeignHandler for Compositor {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.foreign
    }
}

// `delegate_xdg_foreign!`, but for `zxdg_exporter_v2`.

delegate_global_dispatch!(Compositor: [ZxdgExporterV2: ()] => XdgForeignState);
delegate_global_dispatch!(Compositor: [ZxdgImporterV2: ()] => XdgForeignState);
checked!(ZxdgExporterV2: () => XdgForeignState, not_a_window);
delegate_dispatch!(Compositor: [ZxdgImporterV2: ()] => XdgForeignState);
delegate_dispatch!(Compositor: [ZxdgExportedV2: XdgExportedUserData] => XdgForeignState);
delegate_dispatch!(Compositor: [ZxdgImportedV2: XdgImportedUserData] => XdgForeignState);

/// A surface exported that is not a toplevel is refused with the protocol's
/// `invalid_surface`, which ends the program that exported it. The handle it
/// would have been sent is never made: nothing is left behind to refuse
/// whoever it was handed to.
fn not_a_window(exporter: &ZxdgExporterV2, request: &mut zxdg_exporter_v2::Request) -> bool {
    let zxdg_exporter_v2::Request::ExportToplevel { surface, .. } = request else {
        return false;
    };
    if compositor::get_role(surface) == Some(XDG_TOPLEVEL_ROLE) {
        return false;
    }
    exporter.post_error(
        zxdg_exporter_v2::Error::InvalidSurface,
        format!("{} is not an xdg_toplevel", surface.id()),
    );
    true
}
