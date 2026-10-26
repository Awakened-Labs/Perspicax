//! Which programs may use the protocols that reach past their own windows,
//! and which are never given to an agent at all.
//!
//! Most of Wayland confines a client to what it drew. A handful of protocols
//! do not: a taskbar lists and closes other programs' windows, a screenshot
//! tool reads every pixel, a display tool moves monitors, perspicax's own
//! shell protocol can end the session, and its settings protocol hears the
//! next chord pressed. Each of those is granted per protocol,
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
//!
//! Some programs exist so the person can oversee an agent, and one handed
//! to the agent oversees nothing: ennius's review queue, approved by the
//! agent whose writes it holds back. [`Never`] names them. It is known by
//! the executable too, with the same caveat, which here errs the safe way: a
//! name that matches more than was meant only refuses more.

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
    /// `perspicax-settings-v1`: the next chord the person presses, before
    /// any binding takes it. perspicax's own, for its settings app.
    Settings,
}

impl Protocol {
    /// Every protocol, in the order a config file lists them.
    pub const ALL: [Self; 7] = [
        Self::ForeignToplevelList,
        Self::ForeignToplevelManagement,
        Self::Workspace,
        Self::Screencopy,
        Self::OutputManagement,
        Self::Shell,
        Self::Settings,
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
            Self::Settings => "settings",
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

    /// The entry as it was written: a path, or a bare name.
    #[must_use]
    pub fn spelled(&self) -> &str {
        match self {
            Self::Path(written) | Self::Name(written) => written,
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

    /// Whether an agent may start `program`, the first word of a command as
    /// the agent wrote it: `[agent] launch`.
    ///
    /// Stricter than [`Rule::admits`], which knows a client's executable
    /// and matches a name wherever the file is. Here the program has not
    /// started, and its name is the agent's to choose: a name admits only
    /// that same bare word, which the session's `PATH` then finds, and a path
    /// only itself. So `flutter` does not admit `/tmp/x/flutter`. The person
    /// named the program their `PATH` finds, not every file an agent can give
    /// that name.
    #[must_use]
    pub fn launches(&self, program: &str) -> bool {
        match self {
            Self::Off => false,
            Self::Any => true,
            Self::Only(programs) => programs.iter().any(|entry| entry.spelled() == program),
        }
    }

    /// The programs it names, as written, for a refusal to say what is
    /// admitted. None for `Off` or `Any`.
    #[must_use]
    pub fn named(&self) -> Vec<&str> {
        match self {
            Self::Off | Self::Any => Vec::new(),
            Self::Only(programs) => programs.iter().map(Program::spelled).collect(),
        }
    }
}

/// The settings app's program. It edits the session's config, and the config
/// is what holds every agent back, so an agent that could drive it could
/// grant itself anything: it is never an agent's, whatever any list says.
pub const SETTINGS_APP: &str = "perspicax-settings";

/// Whether `program` is [`SETTINGS_APP`]: a bare name or a path, as a config
/// writes one, or the executable a process runs, wherever it is installed
/// and even once an upgrade has replaced its file.
#[must_use]
pub fn is_settings_app(program: &str) -> bool {
    Path::new(unreplaced(program))
        .file_name()
        .is_some_and(|name| name == SETTINGS_APP)
}

/// `exe` as it ran before an upgrade replaced its file, which `/proc` marks
/// with ` (deleted)` after the path.
fn unreplaced(exe: &str) -> &str {
    exe.strip_suffix(" (deleted)").unwrap_or(exe)
}

/// The programs no agent is ever given, whatever grants them: the person's
/// `[agent] never`, above [`SETTINGS_APP`], which no list can take away.
///
/// A program is known by its executable, as for [`Rule::admits`], so a copy
/// under another name is not known. Every doubt otherwise falls on the side
/// of refusing: a name matches that file wherever it is, and a file an
/// upgrade replaced still matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Never(Vec<Program>);

impl Never {
    /// The settings app alone, beneath no list of the person's.
    pub const FLOOR: Self = Self(Vec::new());

    /// These programs, above the settings app.
    #[must_use]
    pub fn new(programs: Vec<Program>) -> Self {
        Self(programs)
    }

    /// The programs it names, as written: the person's list, without the
    /// settings app beneath it.
    #[must_use]
    pub fn named(&self) -> Vec<&str> {
        self.0.iter().map(Program::spelled).collect()
    }

    /// The program, as written, that keeps `exe` from every agent: the
    /// settings app, or the first entry of the list that matches it. `exe`
    /// is the executable a process runs, or the file a command would run;
    /// `None` when it is neither.
    #[must_use]
    pub fn naming(&self, exe: &str) -> Option<&str> {
        let exe = unreplaced(exe);
        if is_settings_app(exe) {
            return Some(SETTINGS_APP);
        }
        self.0
            .iter()
            .find(|program| program.matches(exe))
            .map(Program::spelled)
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

    #[test]
    fn a_bare_name_launches_only_that_word_and_never_a_path_ending_in_it() {
        let rule = Rule::Only(vec![Program::parse("flutter")]);
        assert!(rule.launches("flutter"));
        assert!(
            !rule.launches("/tmp/x/flutter"),
            "a file the agent named so"
        );
        assert!(
            !rule.launches("/usr/bin/flutter"),
            "not even where PATH finds it"
        );
        assert!(!rule.launches("flutter2"));
    }

    #[test]
    fn a_path_launches_only_itself() {
        let rule = Rule::Only(vec![Program::parse("/opt/paradox/bin/paradoxshift")]);
        assert!(rule.launches("/opt/paradox/bin/paradoxshift"));
        assert!(
            !rule.launches("paradoxshift"),
            "a bare word is PATH's to find"
        );
        assert!(!rule.launches("/opt/paradox/bin/../bin/paradoxshift"));
    }

    #[test]
    fn launching_off_admits_nothing_and_any_admits_everything() {
        assert!(!Rule::Off.launches("foot"));
        assert!(Rule::Any.launches("/tmp/x/anything"));
        assert!(Rule::Any.named().is_empty());
        assert_eq!(
            Rule::Only(vec![
                Program::parse("foot"),
                Program::parse("/usr/bin/zenity")
            ])
            .named(),
            ["foot", "/usr/bin/zenity"]
        );
    }

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
    fn the_settings_app_is_never_whatever_the_list_says() {
        for exe in [
            "/usr/bin/perspicax-settings",
            "/home/me/.cargo/bin/perspicax-settings (deleted)",
            "perspicax-settings",
        ] {
            assert_eq!(Never::FLOOR.naming(exe), Some(SETTINGS_APP), "{exe}");
            assert_eq!(
                Never::new(vec![Program::parse("ennius")]).naming(exe),
                Some(SETTINGS_APP),
                "{exe}"
            );
        }
        assert_eq!(
            Never::FLOOR.naming("/usr/bin/perspicax-settings-viewer"),
            None
        );
        assert_eq!(Never::FLOOR.naming("/usr/local/bin/ennius"), None);
    }

    #[test]
    fn a_name_is_never_given_wherever_it_is_and_a_path_only_itself() {
        let never = Never::new(vec![
            Program::parse("ennius"),
            Program::parse("/opt/wallet/bin/wallet"),
        ]);
        assert_eq!(never.naming("/usr/local/bin/ennius"), Some("ennius"));
        assert_eq!(never.naming("/tmp/x/ennius"), Some("ennius"));
        assert_eq!(never.naming("ennius"), Some("ennius"), "a bare word");
        assert_eq!(
            never.naming("/usr/local/bin/ennius (deleted)"),
            Some("ennius"),
            "replaced by an upgrade"
        );
        assert_eq!(never.naming("/usr/local/bin/ennius2"), None);
        assert_eq!(
            never.naming("/opt/wallet/bin/wallet"),
            Some("/opt/wallet/bin/wallet")
        );
        assert_eq!(never.naming("/usr/bin/wallet"), None, "another wallet");
        assert_eq!(never.named(), ["ennius", "/opt/wallet/bin/wallet"]);
        assert!(Never::FLOOR.named().is_empty());
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
