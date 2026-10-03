//! Which programs may use the protocols that reach past their own windows.
//!
//! Most of Wayland confines a client to what it drew. A handful of protocols
//! do not: a taskbar lists and closes other programs' windows, a screenshot
//! tool reads every pixel, a display tool moves monitors, and perspicax's own
//! shell protocol can end the session. Each of those is granted per protocol,
//! in one of three forms: to nobody, to any client, or to the programs named
//! in a list.
//!
//! A program is named by the executable the kernel says it runs, so a name
//! here is evidence and not proof: anything can be called `grim`. A full path
//! is the stricter form, and an interpreter's executable is the interpreter
//! (`python3`), not the script. A client whose executable is unknown, or has
//! been deleted since it started, is admitted only by a rule that admits
//! any client.
//!
//! None of this is about the lock. Whatever a rule says, the compositor keeps
//! every one of these protocols inert while the session is locked.

use std::path::Path;

/// A protocol that reaches past a client's own windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Protocol {
    /// `ext-foreign-toplevel-list-v1`: the windows' titles and app ids,
    /// read-only.
    ForeignToplevelList,
    /// `wlr-foreign-toplevel-management-unstable-v1`: a taskbar, which also
    /// activates, minimizes and closes other programs' windows.
    ForeignToplevelManagement,
    /// `ext-workspace-v1`: a pager, which also switches workspace.
    Workspace,
    /// `wlr-screencopy-unstable-v1`: every pixel on a monitor.
    Screencopy,
    /// `wlr-output-management-unstable-v1`: monitors' modes, scale and
    /// placement.
    OutputManagement,
    /// `perspicax-shell-v1`: the menus a person asks for by key, and ending
    /// the session. perspicax's own, for its desktop shell.
    Shell,
}

impl Protocol {
    /// Every protocol, in the order a config file lists them.
    pub const ALL: [Self; 6] = [
        Self::ForeignToplevelList,
        Self::ForeignToplevelManagement,
        Self::Workspace,
        Self::Screencopy,
        Self::OutputManagement,
        Self::Shell,
    ];

    /// Its key in the `[protocols]` table.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::ForeignToplevelList => "foreign-toplevel-list",
            Self::ForeignToplevelManagement => "foreign-toplevel-management",
            Self::Workspace => "workspace",
            Self::Screencopy => "screencopy",
            Self::OutputManagement => "output-management",
            Self::Shell => "shell",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// A program named in an allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// `/usr/bin/grim`: exactly this executable.
    Path(String),
    /// `grim`: any executable of this name, wherever it is.
    Name(String),
}

impl Program {
    /// A list entry: a path if it has a `/` in it, otherwise a name.
    #[must_use]
    pub fn parse(entry: &str) -> Self {
        if entry.contains('/') {
            Self::Path(entry.to_owned())
        } else {
            Self::Name(entry.to_owned())
        }
    }

    fn matches(&self, exe: &str) -> bool {
        match self {
            Self::Path(path) => exe == path,
            Self::Name(name) => Path::new(exe)
                .file_name()
                .is_some_and(|file| file == name.as_str()),
        }
    }
}

/// Who may use one protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// Nobody: the global is not advertised at all.
    Off,
    /// Any client.
    Any,
    /// Only these programs. Never empty; an empty list is written `Off`.
    Only(Vec<Program>),
}

impl Rule {
    /// Whether a client running `exe` may use the protocol. `None` is a
    /// client whose executable is unknown.
    #[must_use]
    pub fn admits(&self, exe: Option<&str>) -> bool {
        match self {
            Self::Off => false,
            Self::Any => true,
            Self::Only(programs) => exe
                .filter(|exe| !exe.ends_with(" (deleted)"))
                .is_some_and(|exe| programs.iter().any(|program| program.matches(exe))),
        }
    }
}

/// A rule for every protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    rules: [Rule; Protocol::ALL.len()],
}

impl Access {
    /// Every protocol open to any client: what a headless compositor, which
    /// hosts only the programs it was told to start, grants.
    #[must_use]
    pub fn open() -> Self {
        Self {
            rules: [const { Rule::Any }; Protocol::ALL.len()],
        }
    }

    /// This, with `protocol`'s rule replaced.
    #[must_use]
    pub fn with(mut self, protocol: Protocol, rule: Rule) -> Self {
        self.rules[protocol.index()] = rule;
        self
    }

    /// The rule for `protocol`.
    #[must_use]
    pub fn rule(&self, protocol: Protocol) -> &Rule {
        &self.rules[protocol.index()]
    }

    /// Whether a client running `exe` may use `protocol`.
    #[must_use]
    pub fn admits(&self, protocol: Protocol, exe: Option<&str>) -> bool {
        self.rule(protocol).admits(exe)
    }

    /// The protocols `new` may admit fewer clients to than this does, whose
    /// existing users therefore have to be checked again. Conservative: any
    /// change to a rule other than `Any` counts.
    #[must_use]
    pub fn narrowed(&self, new: &Self) -> Vec<Protocol> {
        Protocol::ALL
            .into_iter()
            .filter(|&protocol| {
                let rule = new.rule(protocol);
                rule != self.rule(protocol) && *rule != Rule::Any
            })
            .collect()
    }
}

impl Default for Access {
    fn default() -> Self {
        Self::open()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(entries: &[&str]) -> Rule {
        Rule::Only(entries.iter().map(|entry| Program::parse(entry)).collect())
    }

    #[test]
    fn a_name_matches_that_executable_anywhere() {
        let rule = only(&["grim"]);
        assert!(rule.admits(Some("/usr/bin/grim")));
        assert!(rule.admits(Some("/home/me/.local/bin/grim")));
        assert!(!rule.admits(Some("/usr/bin/grimace")));
        assert!(!rule.admits(Some("/usr/bin/grim/not-grim")));
    }

    #[test]
    fn a_path_matches_only_that_executable() {
        let rule = only(&["/usr/bin/grim"]);
        assert!(rule.admits(Some("/usr/bin/grim")));
        assert!(!rule.admits(Some("/tmp/grim")));
    }

    #[test]
    fn an_unknown_or_deleted_executable_is_admitted_only_by_any() {
        let rule = only(&["grim"]);
        assert!(!rule.admits(None));
        assert!(!rule.admits(Some("/usr/bin/grim (deleted)")));
        assert!(Rule::Any.admits(None));
        assert!(!Rule::Off.admits(Some("/usr/bin/grim")));
    }

    #[test]
    fn each_protocol_has_its_own_rule() {
        let access = Access::open().with(Protocol::Screencopy, Rule::Off);
        assert!(!access.admits(Protocol::Screencopy, Some("/usr/bin/grim")));
        assert!(access.admits(Protocol::Workspace, Some("/usr/bin/waybar")));
    }

    #[test]
    fn narrowing_names_what_changed_to_something_short_of_any() {
        let before = Access::open().with(Protocol::Screencopy, only(&["grim"]));
        let after = Access::open()
            .with(Protocol::Screencopy, Rule::Any)
            .with(Protocol::Workspace, Rule::Off)
            .with(Protocol::OutputManagement, only(&["kanshi"]));
        assert_eq!(
            before.narrowed(&after),
            [Protocol::Workspace, Protocol::OutputManagement]
        );
        assert!(after.narrowed(&after).is_empty());
    }

    #[test]
    fn keys_are_distinct_and_in_order() {
        let keys: Vec<_> = Protocol::ALL
            .iter()
            .map(|protocol| protocol.key())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(keys.len(), sorted.len());
        for (index, protocol) in Protocol::ALL.into_iter().enumerate() {
            assert_eq!(protocol.index(), index);
        }
    }
}
