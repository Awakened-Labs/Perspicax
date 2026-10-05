//! `com.canonical.dbusmenu`: a status icon's menu, read as its program lays
//! it out, and a choice in it told back.
//!
//! The program is told its menu is about to be shown, which is when many
//! fill it, and the whole of it is read in one call. A submenu still empty
//! then is one its program fills only when it is about to be shown, as Qt's
//! do; each is told so, and the menu read once more.

use std::collections::HashMap;

use zbus::{
    Connection,
    zvariant::{OwnedValue, Value},
};

use super::{plain, text};
use crate::model::{
    menu::{Mark, Menu},
    tray::{self, Node},
};

const INTERFACE: &str = "com.canonical.dbusmenu";

/// What GetLayout answers: the layout's revision, and its root item, an
/// `(ia{sv}av)` whose children are items of the same shape, each in a
/// variant.
type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

/// Read the menu served at `path` on `destination`.
pub(super) async fn read(
    connection: &Connection,
    destination: &str,
    path: &str,
) -> zbus::Result<Menu> {
    about_to_show(connection, destination, path, 0).await;
    let mut root = layout(connection, destination, path).await?;
    let unfilled = root.unfilled();
    if !unfilled.is_empty() {
        for id in unfilled {
            about_to_show(connection, destination, path, id).await;
        }
        root = layout(connection, destination, path).await?;
    }
    Ok(tray::menu(&root))
}

/// Tell the program of the menu at `path` on `destination` that its item
/// `id` was chosen.
pub(super) async fn clicked(
    connection: &Connection,
    destination: &str,
    path: &str,
    id: i32,
) -> zbus::Result<()> {
    connection
        .call_method(
            Some(destination),
            path,
            Some(INTERFACE),
            "Event",
            &(id, "clicked", Value::from(0i32), 0u32),
        )
        .await?;
    Ok(())
}

/// Tell the program that item `id`'s submenu, or the menu for 0, is about
/// to be shown. Whether it changed anything is not asked: the layout is
/// read after either way, and a program that does not answer is read as it
/// is.
async fn about_to_show(connection: &Connection, destination: &str, path: &str, id: i32) {
    if let Err(error) = connection
        .call_method(Some(destination), path, Some(INTERFACE), "AboutToShow", &id)
        .await
    {
        tracing::debug!(destination, id, "AboutToShow was not answered: {error}");
    }
}

/// The whole of the menu at `path` on `destination`, as laid out now.
async fn layout(connection: &Connection, destination: &str, path: &str) -> zbus::Result<Node> {
    let reply = connection
        .call_method(
            Some(destination),
            path,
            Some(INTERFACE),
            "GetLayout",
            &(0i32, -1i32, Vec::<&str>::new()),
        )
        .await?;
    Ok(root(&reply.body().deserialize()?))
}

/// The root item of `layout`.
fn root((_revision, (id, properties, children)): &Layout) -> Node {
    let properties = properties
        .iter()
        .map(|(key, value)| (key.as_str(), plain(value)));
    node(*id, properties, children.iter().map(|child| &**child))
}

/// An item of the layout: its `id`, `properties`, and the `children` of
/// its submenu, each an `(ia{sv}av)` structure. What is not said of it is
/// dbusmenu's default; a child that is not an item is left out.
fn node<'v>(
    id: i32,
    properties: impl Iterator<Item = (&'v str, &'v Value<'v>)>,
    children: impl Iterator<Item = &'v Value<'v>>,
) -> Node {
    let mut node = Node {
        id,
        ..Node::default()
    };
    let (mut toggle, mut on) = (None, false);
    for (key, value) in properties {
        match (key, plain(value)) {
            ("type", value) => node.separator = text(value).as_deref() == Some("separator"),
            ("label", value) => node.label = text(value).unwrap_or_default(),
            ("enabled", Value::Bool(enabled)) => node.enabled = *enabled,
            ("visible", Value::Bool(visible)) => node.visible = *visible,
            ("icon-name", value) => node.icon = text(value).filter(|name| !name.is_empty()),
            ("toggle-type", value) => toggle = text(value),
            ("toggle-state", Value::I32(state)) => on = *state == 1,
            ("children-display", value) => {
                node.submenu = text(value).as_deref() == Some("submenu");
            }
            _ => {}
        }
    }
    node.mark = match toggle.as_deref() {
        Some("checkmark") => Some(Mark::Check(on)),
        Some("radio") => Some(Mark::Radio(on)),
        _ => None,
    };
    node.children = children.filter_map(child).collect();
    node
}

/// A child of the layout, as an `(ia{sv}av)` structure.
fn child<'v>(value: &'v Value<'v>) -> Option<Node> {
    let Value::Structure(child) = plain(value) else {
        return None;
    };
    let [
        Value::I32(id),
        Value::Dict(properties),
        Value::Array(children),
    ] = child.fields()
    else {
        return None;
    };
    let properties = properties.iter().filter_map(|(key, value)| match key {
        Value::Str(key) => Some((key.as_str(), value)),
        _ => None,
    });
    Some(node(*id, properties, children.iter()))
}

#[cfg(test)]
mod tests {
    use zbus::zvariant::{
        Endian,
        serialized::{Context, Data},
        to_bytes,
    };

    use super::*;

    /// A layout's item: `(ia{sv}av)`.
    type Wire = (i32, HashMap<String, Value<'static>>, Vec<Value<'static>>);

    fn wire(id: i32, said: &[(&str, Value<'static>)], children: Vec<Wire>) -> Wire {
        (
            id,
            said.iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            children.into_iter().map(Value::from).collect(),
        )
    }

    #[test]
    fn a_layout_is_read_as_it_comes_off_the_bus() {
        let layout = wire(
            0,
            &[("children-display", "submenu".into())],
            vec![
                wire(1, &[("label", "_Open".into())], Vec::new()),
                wire(2, &[("type", "separator".into())], Vec::new()),
                wire(
                    3,
                    &[
                        ("label", "Mute".into()),
                        ("toggle-type", "checkmark".into()),
                        ("toggle-state", 1i32.into()),
                        ("enabled", false.into()),
                    ],
                    Vec::new(),
                ),
                wire(
                    4,
                    &[
                        ("label", "More".into()),
                        ("children-display", "submenu".into()),
                    ],
                    vec![wire(
                        5,
                        &[("label", "Deep".into()), ("icon-name", "go-down".into())],
                        Vec::new(),
                    )],
                ),
            ],
        );
        // Through the bus's encoding and back, as GetLayout's reply is.
        let context = Context::new_dbus(Endian::Little, 0);
        let bytes: Data<'static, 'static> = to_bytes(context, &(7u32, layout)).expect("encoded");
        let (layout, _): (Layout, _) = bytes.deserialize().expect("decoded");
        let root = root(&layout);

        let label = |node: &Node| (node.id, node.label.clone());
        assert!(root.submenu);
        assert_eq!(
            root.children.iter().map(label).collect::<Vec<_>>(),
            [
                (1, "_Open".to_owned()),
                (2, String::new()),
                (3, "Mute".to_owned()),
                (4, "More".to_owned()),
            ]
        );
        assert!(root.children[1].separator);
        let mute = &root.children[2];
        assert_eq!(mute.mark, Some(Mark::Check(true)));
        assert!(!mute.enabled);
        assert!(
            root.children[0].enabled && root.children[0].visible,
            "by default"
        );
        let more = &root.children[3];
        assert!(more.submenu);
        assert_eq!(more.children.len(), 1);
        assert_eq!(more.children[0].icon.as_deref(), Some("go-down"));
    }
}
