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
//! A path is not proof on its own either. The kernel names the file a
//! process runs by the path its own mounts give it, and a program needs no
//! root to have mounts of its own (`unshare`, a rootless container, a
//! sandbox): inside them, any file can be at `/usr/bin/grim`. So a path
//! admits a client only when the file at that path, as the compositor sees
//! it, is the very file the client runs ([`Exe::here`]). And a program
//! [`Program::Installed`] admits must be where only the system could have put
//! it ([`installed`]).
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

/// What the compositor knows of the file a client runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exe<'a> {
    /// The path the kernel names it by, as the client's own mounts see it.
    pub path: &'a str,
    /// Whether the file at `path`, as the compositor sees it, is the very
    /// file the client runs, and not another put there by mounts of its own.
    pub here: bool,
    /// Whether it is, and only the system could have put it there: see
    /// [`installed`]. Never without [`Self::here`].
    pub installed: bool,
}

impl<'a> Exe<'a> {
    /// A client running the file at `path`, somewhere the person could have
    /// written: a program of their own, or a test's.
    #[must_use]
    pub const fn persons(path: &'a str) -> Self {
        Self {
            path,
            here: true,
            installed: false,
        }
    }

    /// A client running the file at `path`, which the system installed.
    #[must_use]
    pub const fn systems(path: &'a str) -> Self {
        Self {
            path,
            here: true,
            installed: true,
        }
    }

    /// Its file's name: `grim` for `/usr/bin/grim`.
    fn file_name(&self) -> Option<&'a str> {
        Path::new(self.path).file_name()?.to_str()
    }
}

/// A program named in an allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// `/usr/bin/grim`: exactly this executable.
    Path(String),
    /// `grim`: any executable of this name, wherever it is.
    Name(String),
    /// `swaylock`, as `[protocols] session-lock` reads a name: an executable
    /// of this name that only the system could have put where it is.
    Installed(String),
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

    /// This, with a bare name read as the system's copy of that program.
    #[must_use]
    pub fn installed(self) -> Self {
        match self {
            Self::Name(name) => Self::Installed(name),
            other => other,
        }
    }

    /// The entry as it was written: a path, or a bare name.
    #[must_use]
    pub fn spelled(&self) -> &str {
        match self {
            Self::Path(written) | Self::Name(written) | Self::Installed(written) => written,
        }
    }

    /// Whether a client running `exe` is this program.
    fn admits(&self, exe: Exe<'_>) -> bool {
        match self {
            Self::Path(path) => exe.here && exe.path == path,
            Self::Name(name) => exe.file_name() == Some(name.as_str()),
            Self::Installed(name) => {
                exe.here && exe.installed && exe.file_name() == Some(name.as_str())
            }
        }
    }

    /// Whether `exe`, a path alone, names this program: what [`Never`]
    /// asks, for which matching more than was meant only refuses more.
    fn matches(&self, exe: &str) -> bool {
        match self {
            Self::Path(path) => exe == path,
            Self::Name(name) | Self::Installed(name) => Path::new(exe)
                .file_name()
                .is_some_and(|file| file == name.as_str()),
        }
    }
}

/// The owner and mode of one entry in a path: what [`installed`] asks of
/// each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    /// The user it belongs to.
    pub uid: u32,
    /// The group it belongs to.
    pub gid: u32,
    /// Its permission bits, as `stat` gives them.
    pub mode: u32,
}

/// Root's uid.
const ROOT: u32 = 0;

impl Owner {
    /// Whether `person` cannot write it: it is root's, nobody else may write
    /// it, and its group may only if the person is not in that group.
    fn kept_from(self, person: &Person) -> bool {
        const GROUP_WRITES: u32 = 0o020;
        const OTHERS_WRITE: u32 = 0o002;
        self.uid == ROOT
            && self.mode & OTHERS_WRITE == 0
            && (self.mode & GROUP_WRITES == 0 || !person.groups.contains(&self.gid))
    }
}

/// Whom a session runs as: the uid and the groups whose writes a file might
/// be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Its effective uid.
    pub uid: u32,
    /// Its own group, then the others it is in.
    pub groups: Vec<u32>,
}

impl Person {
    /// The person a process runs as, from its `/proc/<pid>/status`: the
    /// effective uid and gid, and the supplementary groups.
    #[must_use]
    pub fn from_status(status: &str) -> Option<Self> {
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        };
        let effective = |name| field(name)?.split_whitespace().nth(1)?.parse().ok();
        let uid = effective("Uid")?;
        let gid = effective("Gid")?;
        let mut groups = vec![gid];
        for group in field("Groups")?.split_whitespace() {
            let group = group.parse().ok()?;
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        Some(Self { uid, groups })
    }
}

/// Whether only the system could have put the file at `path` there: it, and
/// every folder above it to `/`, is one [`Owner::kept_from`] `person`. `stat`
/// says who an entry belongs to, without following a link, or `None` for one
/// that is not there.
///
/// `path` is the file's own, every link already followed, as the kernel names
/// a process's executable: a link in it is judged as itself, and its mode
/// lets everyone write it. Nothing is installed by the system for a person
/// who is root, whose every write would pass.
#[must_use]
pub fn installed(path: &Path, person: &Person, stat: impl Fn(&Path) -> Option<Owner>) -> bool {
    use std::path::Component;
    let relative = |component| matches!(component, Component::CurDir | Component::ParentDir);
    if person.uid == ROOT || !path.is_absolute() || path.components().any(relative) {
        return false;
    }
    path.ancestors()
        .all(|entry| stat(entry).is_some_and(|owner| owner.kept_from(person)))
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
    pub fn admits(&self, exe: Option<Exe<'_>>) -> bool {
        match self {
            Self::Off => false,
            Self::Any => true,
            Self::Only(programs) => exe
                .filter(|exe| !exe.path.ends_with(" (deleted)"))
                .is_some_and(|exe| programs.iter().any(|program| program.admits(exe))),
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
    pub fn admits(&self, protocol: Protocol, exe: Option<Exe<'_>>) -> bool {
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
        assert!(rule.admits(Some(Exe::persons("/usr/bin/grim"))));
        assert!(rule.admits(Some(Exe::persons("/home/me/.local/bin/grim"))));
        assert!(!rule.admits(Some(Exe::persons("/usr/bin/grimace"))));
        assert!(!rule.admits(Some(Exe::persons("/usr/bin/grim/not-grim"))));
    }

    #[test]
    fn a_path_matches_only_that_executable() {
        let rule = only(&["/usr/bin/grim"]);
        assert!(rule.admits(Some(Exe::persons("/usr/bin/grim"))));
        assert!(!rule.admits(Some(Exe::persons("/tmp/grim"))));
    }

    /// A program in mounts of its own reads as whatever path it put its file
    /// at: the path is only the path.
    #[test]
    fn a_path_admits_only_the_file_that_is_there() {
        let rule = only(&["/usr/bin/grim"]);
        let elsewhere = Exe {
            path: "/usr/bin/grim",
            here: false,
            installed: false,
        };
        assert!(!rule.admits(Some(elsewhere)));
        assert!(rule.admits(Some(Exe::systems("/usr/bin/grim"))));
        assert!(
            only(&["grim"]).admits(Some(elsewhere)),
            "a name was never more than a name"
        );
    }

    #[test]
    fn an_installed_name_admits_only_the_systems_copy() {
        let rule = Rule::Only(vec![Program::parse("swaylock").installed()]);
        assert!(rule.admits(Some(Exe::systems("/usr/bin/swaylock"))));
        assert!(rule.admits(Some(Exe::systems(
            "/nix/store/0abc-swaylock-1.8/bin/swaylock"
        ))));
        assert!(!rule.admits(Some(Exe::persons("/home/me/.local/bin/swaylock"))));
        assert!(!rule.admits(Some(Exe::systems("/usr/bin/swaylock-effects"))));
        assert!(!rule.admits(Some(Exe::systems("/usr/bin/swaylock (deleted)"))));
        let elsewhere = Exe {
            path: "/usr/bin/swaylock",
            here: false,
            installed: true,
        };
        assert!(!rule.admits(Some(elsewhere)), "installed means here too");
    }

    #[test]
    fn only_a_bare_name_is_read_as_installed() {
        assert_eq!(
            Program::parse("swaylock").installed(),
            Program::Installed("swaylock".to_owned())
        );
        assert_eq!(
            Program::parse("/opt/lock/bin/lock").installed(),
            Program::Path("/opt/lock/bin/lock".to_owned())
        );
        assert_eq!(Program::parse("swaylock").installed().spelled(), "swaylock");
        assert_eq!(
            Never::new(vec![Program::parse("swaylock").installed()])
                .naming("/home/me/.local/bin/swaylock"),
            Some("swaylock"),
            "never errs on refusing: a name is enough"
        );
    }

    const ME: u32 = 1000;
    const NIXBLD: u32 = 30_000;

    fn me() -> Person {
        Person {
            uid: ME,
            groups: vec![ME, 10],
        }
    }

    /// A tree of entries, each `(path, uid, gid, mode)`, as `stat` would say.
    fn tree(entries: &'static [(&'static str, u32, u32, u32)]) -> impl Fn(&Path) -> Option<Owner> {
        move |path| {
            entries
                .iter()
                .find(|(entry, ..)| Path::new(entry) == path)
                .map(|&(_, uid, gid, mode)| Owner { uid, gid, mode })
        }
    }

    const SYSTEM: &[(&str, u32, u32, u32)] = &[
        ("/", 0, 0, 0o755),
        ("/usr", 0, 0, 0o755),
        ("/usr/bin", 0, 0, 0o755),
        ("/usr/bin/swaylock", 0, 0, 0o755),
        ("/home", 0, 0, 0o755),
        ("/home/me", ME, ME, 0o700),
        ("/home/me/swaylock", ME, ME, 0o755),
        ("/opt", 0, 0, 0o755),
        ("/opt/x", ME, ME, 0o755),
        ("/opt/x/swaylock", 0, 0, 0o755),
        ("/tmp", 0, 0, 0o1777),
        ("/tmp/swaylock", 0, 0, 0o755),
        ("/srv", 0, 10, 0o775),
        ("/srv/swaylock", 0, 0, 0o755),
        ("/nix", 0, 0, 0o755),
        ("/nix/store", 0, NIXBLD, 0o1775),
        ("/nix/store/swaylock", 0, 0, 0o555),
        ("/usr/bin/lock", 0, 0, 0o777),
    ];

    #[test]
    fn installed_is_roots_all_the_way_up_and_the_persons_nowhere() {
        let stat = tree(SYSTEM);
        let judge = |path: &str| installed(Path::new(path), &me(), &stat);
        assert!(judge("/usr/bin/swaylock"));
        assert!(!judge("/home/me/swaylock"), "the person's own");
        assert!(
            !judge("/opt/x/swaylock"),
            "root's file in the person's folder"
        );
        assert!(!judge("/tmp/swaylock"), "a folder anyone may write");
        assert!(
            !judge("/srv/swaylock"),
            "a folder a group of theirs may write"
        );
        assert!(
            judge("/nix/store/swaylock"),
            "a folder a group they are not in may write"
        );
        assert!(
            !judge("/usr/bin/lock"),
            "a file anyone may write, or a link"
        );
        assert!(!judge("/usr/bin/missing"), "nothing there");
        assert!(!judge("usr/bin/swaylock"), "not a path from the root");
        assert!(
            !judge("/usr/bin/../bin/swaylock"),
            "judged by name, not by where it leads"
        );
        let root = Person {
            uid: 0,
            groups: vec![0],
        };
        assert!(
            !installed(Path::new("/usr/bin/swaylock"), &root, &stat),
            "every write of root's would pass"
        );
    }

    #[test]
    fn a_person_is_read_from_their_status() {
        let status = "Name:\tperspicax\nUmask:\t0022\nState:\tS (sleeping)\n\
                      Uid:\t1000\t1001\t1001\t1001\nGid:\t100\t1002\t1002\t1002\n\
                      FDSize:\t64\nGroups:\t10 18 1002 \n";
        assert_eq!(
            Person::from_status(status),
            Some(Person {
                uid: 1001,
                groups: vec![1002, 10, 18]
            })
        );
        let alone = "Uid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\nGroups:\t\n";
        assert_eq!(
            Person::from_status(alone),
            Some(Person {
                uid: 1000,
                groups: vec![1000]
            })
        );
        assert_eq!(Person::from_status("Uid:\t1000\n"), None);
    }

    #[test]
    fn an_unknown_or_deleted_executable_is_admitted_only_by_any() {
        let rule = only(&["grim"]);
        assert!(!rule.admits(None));
        assert!(!rule.admits(Some(Exe::persons("/usr/bin/grim (deleted)"))));
        assert!(Rule::Any.admits(None));
        assert!(!Rule::Off.admits(Some(Exe::persons("/usr/bin/grim"))));
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
        assert!(!access.admits(Protocol::Screencopy, Some(Exe::persons("/usr/bin/grim"))));
        assert!(access.admits(Protocol::Workspace, Some(Exe::persons("/usr/bin/waybar"))));
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
