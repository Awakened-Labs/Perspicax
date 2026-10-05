//! `org.kde.StatusNotifierItem`: what a program says of its status icon,
//! read in one call, and what a press on the icon asks of it.

use std::{collections::HashMap, path::PathBuf};

use tiny_skia::Pixmap;
use zbus::{
    Connection,
    zvariant::{OwnedValue, Value},
};

use super::{plain, text};
use crate::model::tray::{self, Icon, Item, Press, Status};

/// The interface a status icon is served with.
pub(super) const INTERFACE: &str = "org.kde.StatusNotifierItem";

/// What a menu's path is when a program has none to serve: KDE's word for
/// it, and the root.
const NO_MENU: [&str; 2] = ["/NO_DBUSMENU", "/"];

/// What an icon is called whose program gives it no name at all.
const UNNAMED: &str = "Status icon";

/// A status icon as read: what the tray shows of it, and where its menu is
/// served, if it has one.
pub(super) struct Read {
    pub(super) item: Item,
    pub(super) menu: Option<String>,
}

/// Read the status icon served at `path` on `destination`.
pub(super) async fn read(
    connection: &Connection,
    destination: &str,
    path: &str,
) -> zbus::Result<Read> {
    let reply = connection
        .call_method(
            Some(destination),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &(INTERFACE,),
        )
        .await?;
    let properties: HashMap<String, OwnedValue> = reply.body().deserialize()?;
    Ok(of(&properties))
}

/// Ask the program of the status icon at `path` on `destination` for what
/// `press` is for, the pointer at `at` on the desk. Not for its menu, which
/// is read rather than asked for.
pub(super) async fn press(
    connection: &Connection,
    destination: &str,
    path: &str,
    press: Press,
    at: (i32, i32),
) -> zbus::Result<()> {
    let method = match press {
        Press::Activate => "Activate",
        Press::SecondaryActivate => "SecondaryActivate",
        Press::ContextMenu | Press::Menu => "ContextMenu",
    };
    connection
        .call_method(Some(destination), path, Some(INTERFACE), method, &at)
        .await?;
    Ok(())
}

/// A status icon, as its `properties` describe it. What is missing, or of
/// a type it should not be, is as if not said.
fn of(properties: &HashMap<String, OwnedValue>) -> Read {
    let get = |key: &str| properties.get(key).map(|value| plain(value));
    let said = |key: &str| get(key).and_then(text).filter(|text| !text.is_empty());
    let folder = said("IconThemePath").map(PathBuf::from);
    let icon = |name: &str, pictures: &str| Icon {
        name: said(name),
        folder: folder.clone(),
        pixmaps: get(pictures).map(pixmaps).unwrap_or_default(),
    };
    let tooltip = || match get("ToolTip")? {
        Value::Structure(tip) => tip.fields().get(2).and_then(text),
        _ => None,
    };
    let menu = said("Menu").filter(|path| !NO_MENU.contains(&path.as_str()));
    let name = |text: Option<String>| text.filter(|name| !name.trim().is_empty());
    let item = Item {
        // The tooltip's title is what a person reads on hovering, and is
        // often the program's full name where its title is a short id:
        // VLC's title is "vlc", its tooltip's "VLC media player". A blank
        // one, or none, falls back to the title, then the id.
        title: name(tooltip())
            .or_else(|| name(said("Title")))
            .or_else(|| name(said("Id")))
            .unwrap_or_else(|| UNNAMED.to_owned()),
        status: Status::parse(said("Status").as_deref().unwrap_or_default()),
        icon: icon("IconName", "IconPixmap"),
        attention: icon("AttentionIconName", "AttentionIconPixmap"),
        menu: menu.is_some(),
        only_menu: matches!(get("ItemIsMenu"), Some(Value::Bool(true))),
    };
    Read { item, menu }
}

/// The pictures in an `a(iiay)`, each as premultiplied RGBA; any that is
/// not one left out.
fn pixmaps(value: &Value<'_>) -> Vec<Pixmap> {
    let Value::Array(pictures) = value else {
        return Vec::new();
    };
    pictures
        .iter()
        .filter_map(|picture| {
            let Value::Structure(picture) = plain(picture) else {
                return None;
            };
            let [Value::I32(width), Value::I32(height), Value::Array(bytes)] = picture.fields()
            else {
                return None;
            };
            let bytes = bytes
                .iter()
                .map(|byte| match byte {
                    Value::U8(byte) => Some(*byte),
                    _ => None,
                })
                .collect::<Option<Vec<u8>>>()?;
            tray::pixmap(*width, *height, &bytes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::ObjectPath;

    use super::*;

    /// A picture as a program sends one: a(iiay).
    type Pictures = Vec<(i32, i32, Vec<u8>)>;

    fn properties<const N: usize>(
        said: [(&str, Value<'static>); N],
    ) -> HashMap<String, OwnedValue> {
        said.into_iter()
            .map(|(key, value)| {
                let value = OwnedValue::try_from(value).expect("an owned value");
                (key.to_owned(), value)
            })
            .collect()
    }

    #[test]
    fn a_status_icon_is_read_from_its_properties() {
        let red: Pictures = vec![(1, 1, vec![0xff, 0xff, 0, 0])];
        // What VLC says of itself.
        let tooltip = |title: &str| {
            (
                String::new(),
                Pictures::new(),
                title.to_owned(),
                String::new(),
            )
        };
        let read = of(&properties([
            ("Id", "vlc".into()),
            ("Title", "vlc".into()),
            ("Status", "NeedsAttention".into()),
            ("IconName", "".into()),
            ("IconPixmap", red.into()),
            ("AttentionIconName", "mail-unread".into()),
            ("IconThemePath", "/opt/example/icons".into()),
            ("ToolTip", tooltip("VLC media player").into()),
            (
                "Menu",
                ObjectPath::from_static_str_unchecked("/MenuBar").into(),
            ),
            ("ItemIsMenu", false.into()),
        ]));
        let item = &read.item;
        assert_eq!(item.title, "VLC media player", "its tooltip's title");
        assert_eq!(item.status, Status::NeedsAttention);
        assert_eq!(item.icon.name, None, "an empty name is none");
        assert_eq!(item.icon.pixmaps.len(), 1);
        assert_eq!(item.icon.pixmaps[0].data(), [0xff, 0, 0, 0xff], "red");
        assert_eq!(item.attention.name.as_deref(), Some("mail-unread"));
        assert_eq!(
            item.attention.folder.as_deref(),
            Some(std::path::Path::new("/opt/example/icons"))
        );
        assert_eq!(read.menu.as_deref(), Some("/MenuBar"));
        assert!(item.menu && !item.only_menu);

        for blank in ["", "  "] {
            let read = of(&properties([
                ("Id", "vlc".into()),
                ("Title", "vlc".into()),
                ("ToolTip", tooltip(blank).into()),
            ]));
            assert_eq!(read.item.title, "vlc", "a blank tooltip title: its title");
        }
        let read = of(&properties([("Id", " ".into()), ("Title", "".into())]));
        assert_eq!(read.item.title, UNNAMED, "nothing to go by: still a name");

        let read = of(&properties([
            ("Id", "bare".into()),
            ("Title", "".into()),
            ("Menu", "/NO_DBUSMENU".into()),
            ("ItemIsMenu", "not a boolean".into()),
        ]));
        assert_eq!(read.item.title, "bare", "no title or tooltip: its id");
        assert_eq!(read.item.status, Status::Active);
        assert!(read.menu.is_none() && !read.item.menu, "no menu to read");
        assert!(!read.item.only_menu, "a mistyped property is not said");
    }
}
