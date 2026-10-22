//! A window's menu: what can be made of a window, as a right-click on its
//! task offers it.
//!
//! Sticky, Always on top and Shade are ticked as the window is, and Send to
//! workspace opens the workspaces of its monitor's group, the one it is on
//! dotted; each says what the window is to be, not what to change, so a
//! choice made after a key changed the window still means what the person
//! saw. These come from what perspicax tells its own shell, and a shell with
//! no such channel offers only what the taskbar's protocol can ask:
//! Minimize, or Restore for a window put away; Maximize or Unmaximize; and
//! Close. Shade is greyed out for a window that cannot roll up, one that
//! draws its own frame or is fullscreen, and Send to workspace is there
//! only for a window with a workspace of its own and somewhere else to go.
//! The marks stand where icons would, so the items have none.
//!
//! Nothing here knows of the taskbar: the window and what is told of it
//! are all it is built from, wherever the menu opens.

use super::{
    menu::{Does, Item, Mark, Menu},
    tasks::{Placing, Window},
};

/// What choosing an item of a window's menu asks of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    /// Be on every workspace, or on one.
    Sticky(bool),
    /// Be kept in front of the other windows, or not.
    OnTop(bool),
    /// Be rolled up to its titlebar, or down.
    Shade(bool),
    /// Go to the workspace of this serial in the pager.
    SendTo(u64),
    /// Be put away.
    Minimize,
    /// Come back from being put away, and forward, as a left click on its
    /// task brings it.
    Restore,
    /// Fill the monitor, or not.
    Maximize(bool),
    /// Close, which it may decline.
    Close,
}

/// The menu of `window`, standing as `placing` says if perspicax said,
/// with `workspaces`, by serial and name, where it could be sent.
pub(crate) fn build<O>(
    window: &Window<O>,
    placing: Option<&Placing>,
    workspaces: &[(u64, &str)],
) -> Menu {
    let mut items = Vec::new();
    if let Some(placing) = placing {
        items.push(item(
            "Sticky",
            Verb::Sticky(!placing.sticky),
            placing.homed,
            Some(Mark::Check(placing.sticky)),
        ));
        items.push(item(
            "Always on top",
            Verb::OnTop(!placing.on_top),
            true,
            Some(Mark::Check(placing.on_top)),
        ));
        items.push(item(
            "Shade",
            Verb::Shade(!placing.shaded),
            placing.shadeable,
            Some(Mark::Check(placing.shaded)),
        ));
        if placing.homed && workspaces.len() > 1 {
            let each = workspaces.iter().map(|&(serial, name)| {
                let here = placing.workspace == Some(serial);
                item(name, Verb::SendTo(serial), true, Some(Mark::Radio(here)))
            });
            items.push(Item {
                label: "Send to workspace".to_owned(),
                icon: None,
                keywords: Vec::new(),
                does: Does::Open(Menu {
                    items: each.collect(),
                }),
            });
        }
        items.push(Item::separator());
    }
    items.push(if window.minimized {
        item("Restore", Verb::Restore, true, None)
    } else {
        item("Minimize", Verb::Minimize, true, None)
    });
    items.push(if window.maximized {
        item("Unmaximize", Verb::Maximize(false), true, None)
    } else {
        item("Maximize", Verb::Maximize(true), true, None)
    });
    items.push(Item::separator());
    items.push(item("Close", Verb::Close, true, None));
    Menu { items }
}

/// An item of a window's menu, labelled `label`, that asks `verb` of it.
fn item(label: &str, verb: Verb, enabled: bool, mark: Option<Mark>) -> Item {
    Item {
        label: label.to_owned(),
        icon: None,
        keywords: Vec::new(),
        does: Does::Window {
            verb,
            enabled,
            mark,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A menu as it reads: a tick or a dot, the label, `>` for a submenu,
    /// and `(greyed)`; `--` for a separator.
    fn shown(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| {
                if item.is_separator() {
                    return "--".to_owned();
                }
                let mark = match item.mark() {
                    Some(Mark::Check(true)) => "[x] ",
                    Some(Mark::Check(false)) => "[ ] ",
                    Some(Mark::Radio(true)) => "(*) ",
                    Some(Mark::Radio(false)) => "( ) ",
                    None => "",
                };
                let more = if item.submenu().is_some() { " >" } else { "" };
                let greyed = if item.choosable() { "" } else { " (greyed)" };
                format!("{mark}{}{more}{greyed}", item.label)
            })
            .collect()
    }

    /// What choosing the item labelled `label` asks.
    fn verb(menu: &Menu, label: &str) -> Verb {
        let item = menu.items.iter().find(|item| item.label == label);
        match item.map(|item| &item.does) {
            Some(&Does::Window { verb, .. }) => verb,
            other => panic!("{label}: {other:?}"),
        }
    }

    /// Where a window can be sent: a monitor's group of three.
    const THREE: [(u64, &str); 3] = [(10, "1"), (11, "2"), (12, "3")];

    /// A window on a workspace of its own, framed by perspicax.
    fn placed() -> Placing {
        Placing {
            homed: true,
            shadeable: true,
            workspace: Some(11),
            ..Placing::default()
        }
    }

    #[test]
    fn a_windows_menu_ticks_what_it_is_and_each_item_asks_for_the_other() {
        let placing = Placing {
            on_top: true,
            ..placed()
        };
        let menu = build(&Window::<()>::default(), Some(&placing), &THREE);
        assert_eq!(
            shown(&menu),
            [
                "[ ] Sticky",
                "[x] Always on top",
                "[ ] Shade",
                "Send to workspace >",
                "--",
                "Minimize",
                "Maximize",
                "--",
                "Close",
            ]
        );
        assert_eq!(verb(&menu, "Sticky"), Verb::Sticky(true));
        assert_eq!(verb(&menu, "Always on top"), Verb::OnTop(false));
        assert_eq!(verb(&menu, "Shade"), Verb::Shade(true));
        assert_eq!(verb(&menu, "Minimize"), Verb::Minimize);
        assert_eq!(verb(&menu, "Maximize"), Verb::Maximize(true));
        assert_eq!(verb(&menu, "Close"), Verb::Close);

        let workspaces = menu.items[3].submenu().expect("a submenu");
        assert_eq!(
            shown(workspaces),
            ["( ) 1", "(*) 2", "( ) 3"],
            "the one it is on dotted"
        );
        assert_eq!(verb(workspaces, "3"), Verb::SendTo(12));
        assert_eq!(
            verb(workspaces, "2"),
            Verb::SendTo(11),
            "and to be chosen, which does nothing"
        );
        assert!(
            menu.items.iter().all(|item| item.icon.is_none()),
            "the marks stand where icons would"
        );
    }

    #[test]
    fn a_window_put_away_or_filling_the_monitor_is_offered_back() {
        let window = Window::<()> {
            minimized: true,
            maximized: true,
            ..Window::default()
        };
        let menu = build(&window, Some(&placed()), &THREE);
        assert_eq!(verb(&menu, "Restore"), Verb::Restore);
        assert_eq!(verb(&menu, "Unmaximize"), Verb::Maximize(false));
        let sticky = Placing {
            sticky: true,
            on_top: true,
            shaded: true,
            workspace: None,
            ..placed()
        };
        let menu = build(&window, Some(&sticky), &THREE);
        assert_eq!(
            shown(&menu)[..4],
            [
                "[x] Sticky",
                "[x] Always on top",
                "[x] Shade",
                "Send to workspace >"
            ]
        );
        assert_eq!(verb(&menu, "Sticky"), Verb::Sticky(false));
        assert_eq!(verb(&menu, "Shade"), Verb::Shade(false));
        assert_eq!(
            shown(menu.items[3].submenu().expect("a submenu")),
            ["( ) 1", "( ) 2", "( ) 3"],
            "on every one, so none dotted"
        );
    }

    #[test]
    fn with_nothing_told_but_the_taskbars_a_windows_menu_has_what_that_can_ask() {
        let menu = build(&Window::<()>::default(), None, &THREE);
        assert_eq!(shown(&menu), ["Minimize", "Maximize", "--", "Close"]);
    }

    #[test]
    fn shade_is_greyed_out_for_a_window_that_cannot_roll_up() {
        let placing = Placing {
            shadeable: false,
            ..placed()
        };
        let menu = build(&Window::<()>::default(), Some(&placing), &THREE);
        assert_eq!(shown(&menu)[2], "[ ] Shade (greyed)");
    }

    #[test]
    fn a_window_is_sent_only_from_a_workspace_of_its_own_to_another() {
        let window = Window::<()>::default();
        let alone = build(&window, Some(&placed()), &THREE[1..2]);
        assert_eq!(
            shown(&alone)[..4],
            ["[ ] Sticky", "[ ] Always on top", "[ ] Shade", "--"],
            "nowhere else to go"
        );
        let homeless = Placing {
            homed: false,
            workspace: None,
            ..placed()
        };
        let menu = build(&window, Some(&homeless), &THREE);
        assert_eq!(
            shown(&menu)[..4],
            [
                "[ ] Sticky (greyed)",
                "[ ] Always on top",
                "[ ] Shade",
                "--"
            ],
            "no workspace of its own to leave"
        );
    }
}
