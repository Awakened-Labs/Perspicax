//! What the shell shows, worked out without drawing it or asking Wayland:
//! each part pure, and tested as a value in and a value out.

#[cfg(feature = "panel")]
pub(crate) mod clock;
#[cfg(any(feature = "wallpaper", feature = "menus", feature = "panel"))]
pub(crate) mod image;
#[cfg(feature = "panel")]
pub(crate) mod layouts;
#[cfg(any(feature = "panel", feature = "wallpaper"))]
pub(crate) mod pager;
#[cfg(feature = "panel")]
pub(crate) mod tasks;
#[cfg(feature = "tray")]
pub(crate) mod tray;
#[cfg(feature = "wallpaper")]
pub(crate) mod wallpaper;

// The installed applications and their icons: what the menus list, and
// what the taskbar draws beside each window.
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) mod apps;
#[cfg(feature = "menus")]
pub(crate) mod categories;
#[cfg(feature = "icons")]
pub(crate) mod folder;
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) mod fs;
#[cfg(any(feature = "menus", feature = "panel"))]
pub(crate) mod icons;
#[cfg(feature = "menus")]
pub(crate) mod menu;
#[cfg(feature = "menus")]
pub(crate) mod menu_file;

/// A pointer button, as what the shell shows tells them apart.
#[cfg(any(feature = "menus", feature = "panel"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
    Other,
}
