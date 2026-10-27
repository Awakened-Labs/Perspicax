//! Whether a program that starts MCP servers -- Claude Code, ennius -- starts
//! perspicax's, and how to ask it to.
//!
//! An agent inside the session reaches the desktop through `perspicax attach`,
//! a length of wire to the session's agent socket, run as an MCP server by
//! whatever the agent is. Each such program keeps its servers by name, and a
//! person may have written perspicax's in by hand, under a name of their own,
//! before any install did. So perspicax's is known by what it runs, not by
//! what it is called: the name a person chose, and whatever they granted
//! under it, are theirs to keep.
//!
//! Both are written to through their own command lines, never by editing
//! their files. Each owns the format of what it keeps, checks what it is
//! given in its own words, and may be running and rewriting the file while
//! perspicax is asking.

use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};

/// The name an install gives a server it adds.
pub const NAME: &str = "perspicax";

/// What every server of perspicax's runs, after the program.
pub const ATTACH: &str = "attach";

/// A server as the program that starts it lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What the program calls it.
    pub name: String,
    /// The program it starts, as written.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Whether it is started at all: ennius may switch one off.
    pub enabled: bool,
}

impl Entry {
    /// The command and its arguments, as a person would type them.
    #[must_use]
    pub fn shown(&self) -> String {
        shown(std::iter::once(self.command.as_str()).chain(self.args.iter().map(String::as_str)))
    }
}

/// What one entry is, to perspicax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// `perspicax attach`: perspicax's own. `runs` is the file it starts,
    /// none when its program is not there to run.
    Ours { runs: Option<PathBuf> },
    /// Something else that reaches the desktop: a netcat or a socat on the
    /// session's agent socket, as the README offers. The person's, and kept.
    Bridge,
    /// Anything else.
    Other,
}

impl Kind {
    /// What `entry` is, with bare names found on `path`.
    ///
    /// Perspicax's own when it runs a program called `perspicax`, named so or
    /// reached through a link, with `attach` first: any perspicax will do,
    /// since `attach` is only a length of wire, so a newer one installed over
    /// it changes nothing.
    #[must_use]
    pub fn of(entry: &Entry, path: Option<&OsStr>) -> Self {
        let runs = perspicax_config::which(&entry.command, path);
        let called = |file: &Path| file.file_name() == Some(OsStr::new(NAME));
        let perspicax = called(Path::new(&entry.command))
            || runs
                .as_deref()
                .and_then(|file| file.canonicalize().ok())
                .is_some_and(|file| called(&file));
        if perspicax && entry.args.first().is_some_and(|arg| arg == ATTACH) {
            return Self::Ours { runs };
        }
        let reaches =
            |word: &str| word.contains("PERSPICAX_MCP_SOCKET") || word.contains("perspicax-mcp");
        if std::iter::once(&entry.command)
            .chain(&entry.args)
            .any(|word| reaches(word))
        {
            Self::Bridge
        } else {
            Self::Other
        }
    }
}

/// Whether the servers a program lists include perspicax's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wiring {
    /// Perspicax's, under `name`, starting `runs`.
    Wired {
        name: String,
        runs: PathBuf,
        enabled: bool,
    },
    /// Perspicax's, under the entry's name, but its program is not there to
    /// run: moved, or deleted.
    Broken(Entry),
    /// Another way to the desktop, the person's own.
    Bridged(Entry),
    /// More than one way to the desktop, under these names. The desktop holds
    /// one conversation at a time, so they would take turns at it, and which
    /// to keep -- and with it which name's grants -- is the person's to say.
    Twice(Vec<String>),
    /// No way to the desktop, but [`NAME`] is taken by this, which is not one.
    Taken(Entry),
    /// No way to the desktop.
    Unwired,
}

impl Wiring {
    /// What `entries` come to, with bare names found on `path`.
    #[must_use]
    pub fn of(entries: &[Entry], path: Option<&OsStr>) -> Self {
        let mut ways: Vec<(&Entry, Kind)> = entries
            .iter()
            .map(|entry| (entry, Kind::of(entry, path)))
            .filter(|(_, kind)| *kind != Kind::Other)
            .collect();
        match ways.len() {
            0 => entries
                .iter()
                .find(|entry| entry.name == NAME)
                .map_or(Self::Unwired, |entry| Self::Taken(entry.clone())),
            1 => match ways.remove(0) {
                (entry, Kind::Ours { runs: Some(runs) }) => Self::Wired {
                    name: entry.name.clone(),
                    runs,
                    enabled: entry.enabled,
                },
                (entry, Kind::Ours { runs: None }) => Self::Broken(entry.clone()),
                (entry, _) => Self::Bridged(entry.clone()),
            },
            _ => Self::Twice(ways.iter().map(|(entry, _)| entry.name.clone()).collect()),
        }
    }
}

/// The program to register: the perspicax on `path` that is this one,
/// `this`, so that a link into a folder an upgrade replaces is followed again
/// at every start; else `this` itself. And whether it was on `path` at all,
/// which a build run from where `cargo` left it is not.
#[must_use]
pub fn registered(this: &Path, path: Option<&OsStr>) -> (PathBuf, bool) {
    let itself = this.canonicalize().ok();
    let on_path = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(NAME))
        .find(|candidate| itself.is_some() && candidate.canonicalize().ok() == itself);
    match on_path {
        Some(program) => (program, true),
        None => (this.to_owned(), false),
    }
}

/// Words as a person would type them into a shell: each that a shell would
/// split or expand, in single quotes.
pub fn shown<'a>(words: impl IntoIterator<Item = &'a str>) -> String {
    let plain = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c))
    };
    words
        .into_iter()
        .map(|word| {
            if plain(word) {
                word.to_owned()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How long a program is given to answer before it is stopped.
const PATIENCE: Duration = Duration::from_secs(10);

/// What a program said, and how it ended.
#[derive(Debug)]
pub struct Answer {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl Answer {
    /// Its own words on what went wrong: the first line of its stderr, or of
    /// its stdout when it said nothing there, or its exit status.
    #[must_use]
    pub fn complaint(&self) -> String {
        [&self.stderr, &self.stdout]
            .iter()
            .find_map(|said| said.lines().map(str::trim).find(|line| !line.is_empty()))
            .map_or_else(|| format!("it ended with {}", self.status), str::to_owned)
    }
}

/// Run `program` with `args` and `env` added to this process's environment,
/// reading nothing, and keep what it says: its stdout and stderr are its
/// answer, never this process's output. One still running after
/// [`PATIENCE`] is killed, and that is an error.
///
/// # Errors
///
/// It could not be started, or did not end in time.
pub fn ask(program: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Answer> {
    let mut child = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", program.display()))?;
    // Read as it writes, so that a full pipe cannot stop it short of ending.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            bail!(
                "{} did not answer in {} seconds, and was stopped",
                program.display(),
                PATIENCE.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    Ok(Answer {
        status,
        stdout: heard(&stdout),
        stderr: heard(&stderr),
    })
}

/// Read `pipe` to its end on a thread of its own, and say what it held.
fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (tell, told) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        thread::spawn(move || {
            let mut said = Vec::new();
            pipe.read_to_end(&mut said).ok();
            tell.send(String::from_utf8_lossy(&said).into_owned()).ok();
        });
    }
    told
}

/// What a pipe held, once its program has ended. A pipe a program it started
/// still holds open never ends, so it is waited for only briefly, and what it
/// held is then lost rather than waited for.
fn heard(told: &mpsc::Receiver<String>) -> String {
    told.recv_timeout(Duration::from_secs(1))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A fresh folder for one test, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("perspicax-wiring-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("a scratch folder");
            Self(dir)
        }

        /// A program in it at `name`, which may be run: written as
        /// [`crate::forked`] says.
        fn program(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            crate::forked::program(&path, "#!/bin/sh\n");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn entry(name: &str, command: &str, args: &[&str]) -> Entry {
        Entry {
            name: name.to_owned(),
            command: command.to_owned(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            enabled: true,
        }
    }

    #[test]
    fn perspicax_attach_is_wired_whatever_its_name() {
        let scratch = Scratch::new("named");
        let program = scratch.program("bin/perspicax");
        let path = std::env::join_paths([program.parent().unwrap()]).unwrap();
        // dave's own, from before any install, with its typo.
        let wiring = Wiring::of(&[entry("perpicax", "perspicax", &["attach"])], Some(&path));
        assert_eq!(
            wiring,
            Wiring::Wired {
                name: "perpicax".to_owned(),
                runs: program.clone(),
                enabled: true
            }
        );
        let named = program.to_str().unwrap();
        assert!(matches!(
            Wiring::of(
                &[entry(
                    "perspicax",
                    named,
                    &["attach", "/run/user/1000/perspicax-mcp"]
                )],
                None
            ),
            Wiring::Wired { .. }
        ));
    }

    #[test]
    fn a_link_to_perspicax_is_perspicax() {
        let scratch = Scratch::new("link");
        let program = scratch.program("versions/perspicax");
        let link = scratch.0.join("px");
        std::os::unix::fs::symlink(&program, &link).expect("a link");
        let wiring = Wiring::of(
            &[entry("desktop", link.to_str().unwrap(), &["attach"])],
            None,
        );
        assert!(matches!(wiring, Wiring::Wired { name, .. } if name == "desktop"));
    }

    #[test]
    fn a_perspicax_that_is_gone_is_broken() {
        let scratch = Scratch::new("gone");
        let gone = scratch.0.join("bin/perspicax");
        let broken = entry("perspicax", gone.to_str().unwrap(), &["attach"]);
        assert_eq!(
            Wiring::of(std::slice::from_ref(&broken), None),
            Wiring::Broken(broken)
        );
    }

    #[test]
    fn a_netcat_on_the_agent_socket_is_the_persons_way_in() {
        let nc = entry("desktop", "nc", &["-N", "-U", "${PERSPICAX_MCP_SOCKET}"]);
        assert_eq!(
            Wiring::of(std::slice::from_ref(&nc), None),
            Wiring::Bridged(nc)
        );
    }

    #[test]
    fn two_ways_in_are_named_and_none_is_chosen() {
        let both = [
            entry("perpicax", "perspicax", &["attach"]),
            entry("github", "npx", &["-y", "server-github"]),
            entry(
                "desktop",
                "socat",
                &["STDIO", "UNIX-CONNECT:/run/user/1000/perspicax-mcp"],
            ),
        ];
        assert_eq!(
            Wiring::of(&both, None),
            Wiring::Twice(vec!["perpicax".to_owned(), "desktop".to_owned()])
        );
    }

    #[test]
    fn a_perspicax_that_is_not_attached_is_not_a_way_in_but_takes_the_name() {
        let headless = entry("perspicax", "perspicax", &["--headless", "--mcp"]);
        assert_eq!(
            Wiring::of(std::slice::from_ref(&headless), None),
            Wiring::Taken(headless)
        );
        let other = entry("headless", "perspicax", &["--headless", "--mcp"]);
        assert_eq!(Wiring::of(&[other], None), Wiring::Unwired);
        assert_eq!(Wiring::of(&[], None), Wiring::Unwired);
    }

    #[test]
    fn the_perspicax_registered_is_the_one_on_path_that_is_this_one() {
        let scratch = Scratch::new("registered");
        let real = scratch.program("versions/1/perspicax");
        fs::create_dir_all(scratch.0.join("bin")).unwrap();
        let link = scratch.0.join("bin/perspicax");
        std::os::unix::fs::symlink(&real, &link).expect("a link");
        let path = std::env::join_paths([scratch.0.join("bin")]).unwrap();
        assert_eq!(registered(&real, Some(&path)), (link, true));
        let elsewhere = scratch.program("target/debug/perspicax");
        assert_eq!(
            registered(&elsewhere, Some(&path)),
            (elsewhere.clone(), false),
            "a build that is not the one on PATH registers itself, and says so"
        );
    }

    #[test]
    fn words_a_shell_would_split_are_quoted() {
        assert_eq!(
            shown([
                "claude",
                "mcp",
                "add-json",
                "-s",
                "user",
                "perspicax",
                r#"{"type":"stdio"}"#
            ]),
            r#"claude mcp add-json -s user perspicax '{"type":"stdio"}'"#
        );
        assert_eq!(shown(["it's"]), r"'it'\''s'");
    }

    #[test]
    fn a_program_is_heard_out_and_its_complaint_is_its_first_line() {
        let scratch = Scratch::new("ask");
        let program = scratch.0.join("complains");
        crate::forked::program(
            &program,
            "#!/bin/sh\necho \"out $1\"\necho \"\n  no such server\nmore\" >&2\nexit 3\n",
        );
        let answer = ask(&program, &["here"], &[]).expect("an answer");
        assert_eq!(answer.status.code(), Some(3));
        assert_eq!(answer.stdout, "out here\n");
        assert_eq!(answer.complaint(), "no such server");
    }

    #[test]
    fn a_program_given_an_environment_has_it() {
        let scratch = Scratch::new("env");
        let program = scratch.0.join("env");
        crate::forked::program(&program, "#!/bin/sh\necho \"$ENNIUS_DISABLE_KEYRING\"\n");
        let answer = ask(&program, &[], &[("ENNIUS_DISABLE_KEYRING", "1")]).expect("an answer");
        assert_eq!(answer.stdout, "1\n");
    }
}
