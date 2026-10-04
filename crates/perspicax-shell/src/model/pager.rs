//! The workspaces, as a pager is told them, and which one a press asks for.
//!
//! The compositor tells of groups of workspaces, each showing on some of
//! the monitors, and of each workspace: its name, its place in the grid,
//! and whether it is the one showing. A panel's pager shows the group on
//! its monitor. perspicax spanning its workspaces across the monitors has
//! one group showing on all of them, so every pager shows the same; with a
//! grid per monitor, each shows its own.
//!
//! The compositor ends each batch of news with a `done`, and the pager is
//! drawn again then, so it never shows a batch half-told.
//!
//! Generic over the handles and the monitors, as the tasks are.

use super::Button;

/// Every group and workspace the compositor has told of.
#[derive(Debug)]
pub(crate) struct Pager<G, W, O> {
    groups: Vec<Group<G, W, O>>,
    workspaces: Vec<Workspace<W>>,
    /// The serial the next workspace is given.
    next: u64,
}

/// A group of workspaces, and the monitors it shows on.
#[derive(Debug)]
struct Group<G, W, O> {
    handle: G,
    outputs: Vec<O>,
    /// Its workspaces, by handle, in the order they joined it.
    workspaces: Vec<W>,
}

/// A workspace, as a pager shows it.
#[derive(Debug)]
pub(crate) struct Workspace<W> {
    pub(crate) handle: W,
    /// Its own for as long as it lasts, and never another's.
    pub(crate) serial: u64,
    pub(crate) name: String,
    /// Its place in the grid as the compositor gives it: for perspicax, its
    /// column and then its row, from 0.
    pub(crate) coordinates: Vec<u32>,
    /// It is the one showing.
    pub(crate) active: bool,
}

/// One thing the compositor told of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Grouped<W, O> {
    /// It shows on this monitor now.
    OutputEnter(O),
    OutputLeave(O),
    WorkspaceEnter(W),
    WorkspaceLeave(W),
    Removed,
}

/// One thing the compositor told of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Told {
    Name(String),
    Coordinates(Vec<u32>),
    /// Whether it is the one showing.
    Active(bool),
    Removed,
}

impl<G, W, O> Default for Pager<G, W, O> {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            workspaces: Vec::new(),
            next: 0,
        }
    }
}

impl<G: PartialEq, W: PartialEq, O: PartialEq> Pager<G, W, O> {
    /// A group the compositor has just told of.
    pub(crate) fn add_group(&mut self, handle: G) {
        self.groups.push(Group {
            handle,
            outputs: Vec::new(),
            workspaces: Vec::new(),
        });
    }

    /// A workspace the compositor has just told of.
    pub(crate) fn add_workspace(&mut self, handle: W) {
        self.workspaces.push(Workspace {
            handle,
            serial: self.next,
            name: String::new(),
            coordinates: Vec::new(),
            active: false,
        });
        self.next += 1;
    }

    /// Take in what the compositor `told` of the group `handle`.
    pub(crate) fn group(&mut self, handle: &G, told: Grouped<W, O>) {
        let Some(at) = self.groups.iter().position(|group| group.handle == *handle) else {
            return;
        };
        let group = &mut self.groups[at];
        match told {
            Grouped::OutputEnter(output) => {
                if !group.outputs.contains(&output) {
                    group.outputs.push(output);
                }
            }
            Grouped::OutputLeave(output) => group.outputs.retain(|on| *on != output),
            Grouped::WorkspaceEnter(workspace) => {
                if !group.workspaces.contains(&workspace) {
                    group.workspaces.push(workspace);
                }
            }
            Grouped::WorkspaceLeave(workspace) => {
                group.workspaces.retain(|in_it| *in_it != workspace)
            }
            Grouped::Removed => {
                self.groups.remove(at);
            }
        }
    }

    /// Take in what the compositor `told` of the workspace `handle`.
    pub(crate) fn workspace(&mut self, handle: &W, told: Told) {
        let Some(at) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.handle == *handle)
        else {
            return;
        };
        let workspace = &mut self.workspaces[at];
        match told {
            Told::Name(name) => workspace.name = name,
            Told::Coordinates(coordinates) => workspace.coordinates = coordinates,
            Told::Active(active) => workspace.active = active,
            Told::Removed => {
                let gone = self.workspaces.remove(at);
                for group in &mut self.groups {
                    group.workspaces.retain(|in_it| *in_it != gone.handle);
                }
            }
        }
    }

    /// The monitor `output` is gone, and no group shows on it any more.
    pub(crate) fn gone(&mut self, output: &O) {
        for group in &mut self.groups {
            group.outputs.retain(|on| on != output);
        }
    }

    /// Forget everything, as when the compositor stops telling.
    pub(crate) fn clear(&mut self) {
        self.groups.clear();
        self.workspaces.clear();
    }

    /// The workspaces a pager on monitor `output` shows, each with its
    /// column and row: those of the group showing there, row by row. A
    /// workspace the compositor gives no place takes the next column along
    /// the first row.
    pub(crate) fn on(&self, output: &O) -> Vec<(&Workspace<W>, (u32, u32))> {
        let Some(group) = self
            .groups
            .iter()
            .find(|group| group.outputs.contains(output))
        else {
            return Vec::new();
        };
        let mut shown: Vec<_> = group
            .workspaces
            .iter()
            .filter_map(|handle| self.workspaces.iter().find(|ws| ws.handle == *handle))
            .enumerate()
            .map(|(index, workspace)| {
                let place = match workspace.coordinates[..] {
                    [column, row, ..] => (column, row),
                    [column] => (column, 0),
                    [] => (index as u32, 0),
                };
                (workspace, place)
            })
            .collect();
        shown.sort_by_key(|&(_, (column, row))| (row, column));
        shown
    }

    /// The workspace pressing `button` on the cell of workspace `serial`
    /// asks to show: that one, for a left click on one not showing.
    pub(crate) fn pressed(&self, serial: u64, button: Button) -> Option<&W> {
        let workspace = self.workspaces.iter().find(|ws| ws.serial == serial)?;
        (button == Button::Left && !workspace.active).then_some(&workspace.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Groups and workspaces by number, on monitors by name.
    type Paged = Pager<u32, u32, &'static str>;

    /// A group, `group`, showing on `outputs`, of `count` workspaces in a
    /// row, numbered from `first`, the first of them showing.
    fn grid(pager: &mut Paged, group: u32, outputs: &[&'static str], first: u32, count: u32) {
        pager.add_group(group);
        for &output in outputs {
            pager.group(&group, Grouped::OutputEnter(output));
        }
        for at in 0..count {
            let handle = first + at;
            pager.add_workspace(handle);
            pager.workspace(&handle, Told::Name((at + 1).to_string()));
            pager.workspace(&handle, Told::Coordinates(vec![at, 0]));
            pager.workspace(&handle, Told::Active(at == 0));
            pager.group(&group, Grouped::WorkspaceEnter(handle));
        }
    }

    fn names(pager: &Paged, output: &'static str) -> Vec<String> {
        pager
            .on(&output)
            .iter()
            .map(|(workspace, _)| workspace.name.clone())
            .collect()
    }

    #[test]
    fn a_panel_shows_the_group_on_its_monitor() {
        let mut spanning = Paged::default();
        grid(&mut spanning, 1, &["DP-1", "HDMI-A-1"], 10, 4);
        assert_eq!(names(&spanning, "DP-1"), ["1", "2", "3", "4"]);
        assert_eq!(names(&spanning, "HDMI-A-1"), names(&spanning, "DP-1"));

        let mut each = Paged::default();
        grid(&mut each, 1, &["DP-1"], 10, 2);
        grid(&mut each, 2, &["HDMI-A-1"], 20, 3);
        assert_eq!(names(&each, "DP-1"), ["1", "2"]);
        assert_eq!(names(&each, "HDMI-A-1"), ["1", "2", "3"]);
        assert!(names(&each, "DP-2").is_empty(), "no group shows there");

        each.group(&2, Grouped::Removed);
        assert!(names(&each, "HDMI-A-1").is_empty());
        spanning.gone(&"DP-1");
        assert!(names(&spanning, "DP-1").is_empty());
        assert_eq!(names(&spanning, "HDMI-A-1").len(), 4);
    }

    #[test]
    fn workspaces_are_shown_row_by_row() {
        let mut pager = Paged::default();
        pager.add_group(1);
        pager.group(&1, Grouped::OutputEnter("DP-1"));
        for (handle, column, row) in [(10, 1, 1), (11, 0, 1), (12, 1, 0), (13, 0, 0)] {
            pager.add_workspace(handle);
            pager.workspace(&handle, Told::Name(handle.to_string()));
            pager.workspace(&handle, Told::Coordinates(vec![column, row]));
            pager.group(&1, Grouped::WorkspaceEnter(handle));
        }
        let shown: Vec<_> = pager
            .on(&"DP-1")
            .iter()
            .map(|(workspace, place)| (workspace.name.clone(), *place))
            .collect();
        assert_eq!(
            shown,
            [
                ("13".to_owned(), (0, 0)),
                ("12".to_owned(), (1, 0)),
                ("11".to_owned(), (0, 1)),
                ("10".to_owned(), (1, 1)),
            ]
        );

        pager.workspace(&12, Told::Removed);
        assert_eq!(names(&pager, "DP-1"), ["13", "11", "10"]);
    }

    #[test]
    fn clicking_a_workspace_switches_to_it() {
        let mut pager = Paged::default();
        grid(&mut pager, 1, &["DP-1"], 10, 2);
        let serial = |name: &str| {
            pager
                .on(&"DP-1")
                .iter()
                .find(|(workspace, _)| workspace.name == name)
                .expect("shown")
                .0
                .serial
        };
        let (first, second) = (serial("1"), serial("2"));
        assert_eq!(pager.pressed(second, Button::Left), Some(&11));
        assert_eq!(
            pager.pressed(first, Button::Left),
            None,
            "the one showing already"
        );
        assert_eq!(pager.pressed(second, Button::Middle), None);

        // The compositor switched, and says so.
        pager.workspace(&10, Told::Active(false));
        pager.workspace(&11, Told::Active(true));
        assert_eq!(pager.pressed(first, Button::Left), Some(&10));
        assert_eq!(pager.pressed(second, Button::Left), None);
    }
}
