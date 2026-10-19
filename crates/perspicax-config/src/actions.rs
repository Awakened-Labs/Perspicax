//! Actions as a person writes them: `"close"`, `"workspace-left"`,
//! `{ spawn = ["foot"] }`, `{ pie = "launchers" }`.
//!
//! Both ways, from one vocabulary. [`Spelled::read`] is how the parser takes
//! a binding's value, and [`spell_action`] is how anything that writes a binding back
//! -- the settings app -- gets the words for an [`Action`]. A name is read and
//! written from the same row of the same table, so the two can never drift
//! into spelling one action two ways.

use std::collections::BTreeSet;

use perspicax_policy::{Action, Direction, Towards, VolumeKey};

/// An action as a binding's value says it: a name, or one of the two tables.
/// Data rather than TOML text, because this crate parses TOML and never
/// writes it: whoever writes the file renders these as it sees fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spelled {
    /// `"close"`, `"workspace-3"`, or `"none"` for no action at all.
    Name(String),
    /// `{ spawn = [...] }`, with `agent = true` when an agent may act on
    /// what it starts.
    Spawn { command: Vec<String>, agent: bool },
    /// `{ pie = "<name>" }`.
    Pie(String),
}

/// Every action that is a name and nothing else, in the order a person is
/// told them.
const NAMED: &[(&str, Action)] = &[
    ("close", Action::Close),
    ("cycle-focus", Action::CycleFocus),
    ("reload", Action::Reload),
    ("toggle-sticky", Action::ToggleSticky),
    ("toggle-maximize", Action::ToggleMaximize),
    ("minimize", Action::Minimize),
    ("shade", Action::Shade),
    ("unshade", Action::Unshade),
    ("toggle-shade", Action::ToggleShade),
    ("next-tab", Action::CycleTab { forward: true }),
    ("previous-tab", Action::CycleTab { forward: false }),
    ("tab-with-previous", Action::TabWithPrevious),
    ("detach-tab", Action::DetachTab),
    ("start-menu", Action::StartMenu),
    ("root-menu", Action::RootMenu),
    ("next-layout", Action::CycleLayout { forward: true }),
    ("previous-layout", Action::CycleLayout { forward: false }),
    ("next-workspace", Action::CycleWorkspace { forward: true }),
    (
        "previous-workspace",
        Action::CycleWorkspace { forward: false },
    ),
    ("opacity-up", Action::OpacityUp),
    ("opacity-down", Action::OpacityDown),
    ("opacity-reset", Action::OpacityReset),
    ("toggle-agent", Action::ToggleAgent),
    ("revoke-agent-all", Action::RevokeAgents),
    ("volume-up", Action::Volume(VolumeKey::Up)),
    ("volume-down", Action::Volume(VolumeKey::Down)),
    ("volume-mute", Action::Volume(VolumeKey::Mute)),
    ("mic-mute", Action::Volume(VolumeKey::MicMute)),
    ("move-to-next-output", Action::MoveToOutput(Towards::Next)),
    (
        "move-to-previous-output",
        Action::MoveToOutput(Towards::Previous),
    ),
];

/// A sided action, made from its side.
type Sided = fn(Direction) -> Action;

/// The actions that take a side, written `<verb>-<side>`: `snap-left`.
const SIDED: &[(&str, Sided)] = &[
    ("move-to-output", to_output),
    ("workspace", Action::Workspace),
    ("send-to-workspace", Action::SendToWorkspace),
    ("carry-to-workspace", Action::CarryToWorkspace),
    ("snap", Action::Snap),
];

/// The sides, as a sided action ends.
const SIDES: &[(&str, Direction)] = &[
    ("left", Direction::Left),
    ("right", Direction::Right),
    ("up", Direction::Up),
    ("down", Direction::Down),
];

/// How many layouts `layout-N` can name: xkb holds four at most, and whether
/// the keymap has that many is the keymap's question.
const LAYOUTS: std::ops::RangeInclusive<u8> = 1..=4;

fn to_output(direction: Direction) -> Action {
    Action::MoveToOutput(Towards::Side(direction))
}

impl Spelled {
    /// The action these words name, `None` for `"none"`. `pies` are the pies
    /// `[shell.pie.menus]` names, which a `{ pie = "..." }` must be one of.
    pub(crate) fn read(self, pies: &BTreeSet<String>) -> Result<Option<Action>, String> {
        Ok(Some(match self {
            Self::Spawn { command, .. } if command.is_empty() => {
                return Err("`spawn` needs a program".to_owned());
            }
            Self::Spawn { command, agent } if agent && crate::is_settings_app(&command[0]) => {
                return Err(crate::no_agent_settings("drop `agent = true`"));
            }
            Self::Spawn { command, agent } => Action::Spawn { command, agent },
            Self::Pie(pie) if pies.contains(&pie) => Action::Pie(pie),
            Self::Pie(pie) if pies.is_empty() => {
                return Err(format!(
                    "there is no pie `{pie}`: write it in `[shell.pie.menus]`"
                ));
            }
            Self::Pie(pie) => {
                let named: Vec<&str> = pies.iter().map(String::as_str).collect();
                return Err(format!(
                    "there is no pie `{pie}`; `[shell.pie.menus]` names {}",
                    named.join(", ")
                ));
            }
            Self::Name(name) if name == "none" => return Ok(None),
            Self::Name(name) => named(&name)
                .or_else(|| sided(&name))
                .or_else(|| numbered(&name))
                .ok_or_else(|| unknown(&name))?,
        }))
    }
}

/// The words for `action`, as a binding's value would say it. `None` for an
/// action no file can write: a spawn of nothing, the settings app spawned
/// for an agent, workspace 0, a fifth layout.
///
/// The match names every variant, so an action added to policy without a
/// spelling here does not compile; each arm then asks the same tables
/// [`Spelled::read`] reads.
#[must_use]
pub fn spell_action(action: &Action) -> Option<Spelled> {
    let name = |text: &str| Some(Spelled::Name(text.to_owned()));
    match action {
        Action::Spawn { command, .. } if command.is_empty() => None,
        Action::Spawn { command, agent } if *agent && crate::is_settings_app(&command[0]) => None,
        Action::Spawn { command, agent } => Some(Spelled::Spawn {
            command: command.clone(),
            agent: *agent,
        }),
        Action::Pie(pie) => Some(Spelled::Pie(pie.clone())),
        Action::GoToWorkspace(0) => None,
        Action::GoToWorkspace(number) => name(&format!("workspace-{number}")),
        Action::Layout(number) if LAYOUTS.contains(number) => name(&format!("layout-{number}")),
        Action::Layout(_) => None,
        Action::Workspace(_)
        | Action::SendToWorkspace(_)
        | Action::CarryToWorkspace(_)
        | Action::Snap(_)
        | Action::MoveToOutput(Towards::Side(_)) => SIDED
            .iter()
            .flat_map(|&(verb, make)| {
                SIDES
                    .iter()
                    .map(move |&(side, direction)| (verb, side, make(direction)))
            })
            .find(|(_, _, made)| made == action)
            .and_then(|(verb, side, _)| name(&format!("{verb}-{side}"))),
        Action::Close
        | Action::CycleFocus
        | Action::MoveToOutput(Towards::Next | Towards::Previous)
        | Action::CycleWorkspace { .. }
        | Action::ToggleSticky
        | Action::ToggleMaximize
        | Action::Minimize
        | Action::Shade
        | Action::Unshade
        | Action::ToggleShade
        | Action::CycleTab { .. }
        | Action::TabWithPrevious
        | Action::DetachTab
        | Action::Reload
        | Action::StartMenu
        | Action::RootMenu
        | Action::CycleLayout { .. }
        | Action::OpacityUp
        | Action::OpacityDown
        | Action::OpacityReset
        | Action::ToggleAgent
        | Action::RevokeAgents
        | Action::Volume(_) => NAMED
            .iter()
            .find(|(_, named)| named == action)
            .and_then(|&(text, _)| name(text)),
    }
}

fn named(name: &str) -> Option<Action> {
    NAMED
        .iter()
        .find(|&&(text, _)| text == name)
        .map(|(_, action)| action.clone())
}

/// An action that takes a side: `workspace-left`, `move-to-output-down`.
fn sided(name: &str) -> Option<Action> {
    let (verb, side) = name.rsplit_once('-')?;
    let &(_, direction) = SIDES.iter().find(|&&(text, _)| text == side)?;
    let &(_, make) = SIDED.iter().find(|&&(text, _)| text == verb)?;
    Some(make(direction))
}

/// `workspace-3`. Any positive number is accepted here: whether the grid has
/// that many is a question for the grid, which a later reload may change.
/// `layout-2`, from 1 to 4.
fn numbered(name: &str) -> Option<Action> {
    if let Some(layout) = name.strip_prefix("layout-") {
        let number: u8 = layout.parse().ok()?;
        return LAYOUTS.contains(&number).then_some(Action::Layout(number));
    }
    let number: u16 = name.strip_prefix("workspace-")?.parse().ok()?;
    (number > 0).then_some(Action::GoToWorkspace(number))
}

/// Why `name` is no action, with every action there is, from the tables.
fn unknown(name: &str) -> String {
    let fixed = NAMED.iter().map(|&(text, _)| text.to_owned());
    let numbered = [
        format!("layout-<{}-{}>", LAYOUTS.start(), LAYOUTS.end()),
        "workspace-<number>".to_owned(),
    ];
    let sided = SIDED.iter().map(|&(verb, _)| format!("{verb}-<side>"));
    let every: Vec<String> = fixed.chain(numbered).chain(sided).collect();
    let sides: Vec<&str> = SIDES.iter().map(|&(side, _)| side).collect();
    format!(
        "`{name}` is not an action; use {}, none, {{ spawn = [...] }}, or \
         {{ pie = \"<name>\" }}, where <side> is {} or {}",
        every.join(", "),
        sides[..sides.len() - 1].join(", "),
        sides[sides.len() - 1],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every action, written out by hand rather than taken from the
    /// tables, so a row missing from a table is a spelling missing here.
    fn every_action() -> Vec<Action> {
        let mut every = vec![
            Action::Close,
            Action::CycleFocus,
            Action::MoveToOutput(Towards::Next),
            Action::MoveToOutput(Towards::Previous),
            Action::CycleWorkspace { forward: true },
            Action::CycleWorkspace { forward: false },
            Action::ToggleSticky,
            Action::ToggleMaximize,
            Action::Minimize,
            Action::Shade,
            Action::Unshade,
            Action::ToggleShade,
            Action::Spawn {
                command: vec!["foot".to_owned()],
                agent: false,
            },
            Action::Spawn {
                command: vec!["foot".to_owned(), "-e".to_owned(), "htop".to_owned()],
                agent: true,
            },
            Action::CycleTab { forward: true },
            Action::CycleTab { forward: false },
            Action::TabWithPrevious,
            Action::DetachTab,
            Action::Reload,
            Action::StartMenu,
            Action::RootMenu,
            Action::Pie("launchers".to_owned()),
            Action::CycleLayout { forward: true },
            Action::CycleLayout { forward: false },
            Action::OpacityUp,
            Action::OpacityDown,
            Action::OpacityReset,
            Action::ToggleAgent,
            Action::RevokeAgents,
            Action::Volume(VolumeKey::Up),
            Action::Volume(VolumeKey::Down),
            Action::Volume(VolumeKey::Mute),
            Action::Volume(VolumeKey::MicMute),
            Action::GoToWorkspace(1),
            Action::GoToWorkspace(12),
        ];
        every.extend((1..=4).map(Action::Layout));
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            every.extend([
                Action::Workspace(direction),
                Action::SendToWorkspace(direction),
                Action::CarryToWorkspace(direction),
                Action::Snap(direction),
                Action::MoveToOutput(Towards::Side(direction)),
            ]);
        }
        every
    }

    /// Which variant `action` is. Naming every one, so a new variant fails to
    /// compile here until [`every_action`] is given one of it.
    fn variant(action: &Action) -> usize {
        match action {
            Action::Close => 0,
            Action::CycleFocus => 1,
            Action::MoveToOutput(_) => 2,
            Action::Workspace(_) => 3,
            Action::GoToWorkspace(_) => 4,
            Action::CycleWorkspace { .. } => 5,
            Action::SendToWorkspace(_) => 6,
            Action::CarryToWorkspace(_) => 7,
            Action::ToggleSticky => 8,
            Action::ToggleMaximize => 9,
            Action::Minimize => 10,
            Action::Shade => 11,
            Action::Unshade => 12,
            Action::ToggleShade => 13,
            Action::Snap(_) => 14,
            Action::Spawn { .. } => 15,
            Action::CycleTab { .. } => 16,
            Action::TabWithPrevious => 17,
            Action::DetachTab => 18,
            Action::Reload => 19,
            Action::StartMenu => 20,
            Action::RootMenu => 21,
            Action::Pie(_) => 22,
            Action::CycleLayout { .. } => 23,
            Action::Layout(_) => 24,
            Action::OpacityUp => 25,
            Action::OpacityDown => 26,
            Action::OpacityReset => 27,
            Action::ToggleAgent => 28,
            Action::RevokeAgents => 29,
            Action::Volume(_) => 30,
        }
    }

    const VARIANTS: usize = 31;

    fn pies() -> BTreeSet<String> {
        BTreeSet::from(["launchers".to_owned()])
    }

    #[test]
    fn every_action_variant_is_sampled() {
        let mut seen: Vec<usize> = every_action().iter().map(variant).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, (0..VARIANTS).collect::<Vec<_>>());
    }

    #[test]
    fn every_action_has_a_spelling_that_reads_back_as_itself() {
        for action in every_action() {
            let spelled =
                spell_action(&action).unwrap_or_else(|| panic!("{action:?} has no spelling"));
            let read = spelled.clone().read(&pies());
            assert_eq!(read, Ok(Some(action.clone())), "{action:?} as {spelled:?}");
        }
    }

    #[test]
    fn every_action_is_spelled_once() {
        let spellings: Vec<Spelled> = every_action().iter().filter_map(spell_action).collect();
        for (at, spelled) in spellings.iter().enumerate() {
            assert!(
                !spellings[..at].contains(spelled),
                "{spelled:?} is two actions' spelling"
            );
        }
    }

    #[test]
    fn a_sided_action_is_written_verb_then_side() {
        assert_eq!(
            spell_action(&Action::Workspace(Direction::Left)),
            Some(Spelled::Name("workspace-left".to_owned()))
        );
        assert_eq!(
            spell_action(&Action::MoveToOutput(Towards::Side(Direction::Down))),
            Some(Spelled::Name("move-to-output-down".to_owned()))
        );
    }

    #[test]
    fn a_spawn_keeps_whether_an_agent_may_act_on_it() {
        let action = Action::Spawn {
            command: vec!["foot".to_owned()],
            agent: true,
        };
        assert_eq!(
            spell_action(&action),
            Some(Spelled::Spawn {
                command: vec!["foot".to_owned()],
                agent: true,
            })
        );
    }

    #[test]
    fn an_action_no_file_can_write_has_no_spelling() {
        for action in [
            Action::GoToWorkspace(0),
            Action::Layout(0),
            Action::Layout(5),
            Action::Spawn {
                command: Vec::new(),
                agent: false,
            },
            Action::Spawn {
                command: vec!["/usr/bin/perspicax-settings".to_owned()],
                agent: true,
            },
        ] {
            assert_eq!(spell_action(&action), None, "{action:?}");
        }
    }

    #[test]
    fn none_reads_as_no_action() {
        assert_eq!(Spelled::Name("none".to_owned()).read(&pies()), Ok(None));
    }

    #[test]
    fn the_unknown_action_message_names_every_action_in_the_table() {
        let error = Spelled::Name("explode".to_owned())
            .read(&pies())
            .unwrap_err();
        assert!(
            error.starts_with("`explode` is not an action; use close, "),
            "{error}"
        );
        for &(text, _) in NAMED {
            assert!(error.contains(&format!(" {text},")), "{text}: {error}");
        }
        for &(verb, _) in SIDED {
            assert!(error.contains(&format!("{verb}-<side>")), "{verb}: {error}");
        }
        assert!(error.contains("layout-<1-4>"), "{error}");
        assert!(error.contains("workspace-<number>"), "{error}");
        assert!(
            error.ends_with("where <side> is left, right, up or down"),
            "{error}"
        );
    }
}
