//! What the shell shows, worked out without drawing it or asking Wayland:
//! each part pure, and tested as a value in and a value out.

#[cfg(feature = "panel")]
pub(crate) mod clock;
#[cfg(any(feature = "wallpaper", feature = "menus"))]
pub(crate) mod image;
#[cfg(feature = "wallpaper")]
pub(crate) mod wallpaper;

#[cfg(feature = "menus")]
pub(crate) mod apps;
#[cfg(feature = "menus")]
pub(crate) mod categories;
#[cfg(feature = "menus")]
pub(crate) mod desktop;
#[cfg(feature = "menus")]
pub(crate) mod fs;
#[cfg(feature = "menus")]
pub(crate) mod icons;
#[cfg(feature = "menus")]
pub(crate) mod menu;
#[cfg(feature = "menus")]
pub(crate) mod menu_file;
