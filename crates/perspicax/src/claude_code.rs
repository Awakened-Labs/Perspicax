//! `perspicax claude-code`: Claude Code, told how to reach the desktop.
//!
//! A Claude Code in a terminal inside the session reaches the desktop through
//! `perspicax attach`, run as one of its MCP servers. `install` puts that in
//! Claude Code's user config, which every project of the user's reads;
//! `status` says whether it is there, and runs; `uninstall` takes out what is
//! perspicax's, and leaves a way in the person wrote themselves.
//!
//! Written through Claude Code's own `claude mcp`, never by editing the file:
//! the file is Claude Code's, and Claude Code rewrites it as it runs. It is
//! only read here, for `status`, because `claude mcp get` answers by starting
//! the server to see that it does: an `attach`, and inside a session, a
//! conversation with the desktop.
//!
//! The user scope only. A project's own server, in its local or project
//! scope, goes over this one in that project, and is the project's.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};

use crate::{
    report::{Row, Status},
    wiring::{self, ATTACH, Entry, Kind, NAME, Wiring},
};

/// Claude Code's command line.
const CLAUDE: &str = "claude";

/// What every row here is about.
const ROW: &str = "mcp-server";

/// The command that puts this module's warnings right.
pub const INSTALL: &str = "perspicax claude-code install";

/// Where Claude Code keeps its user config, `.claude.json`: in
/// `$CLAUDE_CONFIG_DIR`, or else the home folder, as Claude Code 2.1 has it.
///
/// `var` reads the environment, as [`crate::attach::socket_path`]'s does.
pub fn config_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    ["CLAUDE_CONFIG_DIR", "HOME"]
        .iter()
        .find_map(|name| var(name).filter(|dir| !dir.is_empty()))
        .map(|dir| PathBuf::from(dir).join(".claude.json"))
}

/// What there is of Claude Code on this machine.
enum Looked {
    /// Nothing: no `claude`, and no config.
    Absent,
    /// A config that could not be read, and why.
    Unreadable(String),
    /// Claude Code, and what it says of perspicax.
    Found(Found),
}

struct Found {
    /// Its command line, when it is on `PATH`.
    claude: Option<PathBuf>,
    /// Its user scope's servers, as it wrote them.
    servers: Map<String, Value>,
    /// What those come to.
    wiring: Wiring,
    /// `PATH`, where a bare name is found.
    path: Option<OsString>,
}

fn look(var: impl Fn(&str) -> Option<String>) -> Looked {
    let path = var("PATH").map(OsString::from);
    let claude = perspicax_config::which(CLAUDE, path.as_deref());
    let read = match config_path(&var) {
        Some(config) => read(&config),
        None => Ok(None),
    };
    let servers = match read {
        Ok(Some(servers)) => servers,
        Ok(None) if claude.is_none() => return Looked::Absent,
        Ok(None) => Map::new(),
        Err(error) => return Looked::Unreadable(format!("{error:#}")),
    };
    let wiring = Wiring::of(&entries(&servers), path.as_deref());
    Looked::Found(Found {
        claude,
        servers,
        wiring,
        path,
    })
}

/// The user scope's servers in the config at `path`, as Claude Code wrote
/// them; none when there is no file. A file with nothing in it, or nothing
/// Claude Code would read as servers, has none.
fn read(path: &Path) -> Result<Option<Map<String, Value>>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    if text.trim().is_empty() {
        return Ok(Some(Map::new()));
    }
    let root: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not JSON Claude Code could read", path.display()))?;
    Ok(Some(
        root.get("mcpServers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
    ))
}

/// The servers that could be perspicax's: those Claude Code starts as
/// programs.
fn entries(servers: &Map<String, Value>) -> Vec<Entry> {
    servers
        .iter()
        .filter(|(_, server)| {
            server
                .get("type")
                .and_then(Value::as_str)
                .is_none_or(|kind| kind == "stdio")
        })
        .filter_map(|(name, server)| {
            Some(Entry {
                name: name.clone(),
                command: server.get("command")?.as_str()?.to_owned(),
                args: server
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                enabled: true,
            })
        })
        .collect()
}

/// Whether Claude Code reaches the desktop, as rows: one, in fact.
pub fn status(var: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    vec![said(&look(var))]
}

/// What `looked` says of Claude Code and the desktop.
fn said(looked: &Looked) -> Row {
    match looked {
        Looked::Absent => Row::new(
            ROW,
            Status::Skipped,
            "Claude Code is not installed here: no `claude` on PATH, and no config",
        ),
        Looked::Unreadable(why) => Row::new(ROW, Status::Fail, why),
        Looked::Found(found) => wired(found),
    }
}

/// What `found` says of the desktop.
fn wired(found: &Found) -> Row {
    let unmendable = found.claude.is_none();
    match &found.wiring {
        Wiring::Wired { name, runs, .. } => Row::new(
            ROW,
            Status::Ok,
            format!("wired as `{name}`, which runs {} {ATTACH}", runs.display()),
        ),
        Wiring::Bridged(entry) => Row::new(
            ROW,
            Status::Ok,
            format!(
                "Claude Code reaches the desktop as `{}`, through `{}`",
                entry.name,
                entry.shown()
            ),
        ),
        Wiring::Broken(entry) if unmendable => Row::new(
            ROW,
            Status::Fail,
            format!(
                "`{}` runs {}, which is not there to run, and there is no `claude` on PATH to \
                 mend it with",
                entry.name, entry.command
            ),
        ),
        Wiring::Broken(entry) => Row::new(
            ROW,
            Status::Warn,
            format!(
                "`{}` runs {}, which is not there to run: run `{INSTALL}`",
                entry.name, entry.command
            ),
        ),
        Wiring::Unwired if unmendable => Row::new(
            ROW,
            Status::Fail,
            "Claude Code does not reach the desktop, and there is no `claude` on PATH to tell it \
             how",
        ),
        Wiring::Unwired => Row::new(
            ROW,
            Status::Warn,
            format!("Claude Code does not reach the desktop: run `{INSTALL}`"),
        ),
        Wiring::Twice(names) => Row::new(ROW, Status::Fail, twice(names)),
        Wiring::Taken(entry) => Row::new(
            ROW,
            Status::Fail,
            format!(
                "`{NAME}` is `{}`, which does not reach the desktop: rename or remove it, then \
                 run `{INSTALL}`",
                entry.shown()
            ),
        ),
    }
}

/// Why more than one way to the desktop is left as it is.
fn twice(names: &[String]) -> String {
    let named = names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        "Claude Code reaches the desktop as {named}, and the desktop holds one conversation at a \
         time: keep one, and take out the others with `claude mcp remove -s user NAME`"
    )
}

/// Put perspicax's server in Claude Code's user config, or mend the one
/// there, so that it runs `this` -- or the perspicax on `PATH` that is `this`,
/// [`wiring::registered`] -- with `attach`. Leaves it as it is when it already
/// reaches the desktop, and refuses to choose between two ways in, or to take
/// a name somebody else's server holds. `dry_run` says what it would run, and
/// runs nothing.
pub fn install(this: &Path, dry_run: bool, var: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    let found = match look(var) {
        Looked::Absent => {
            return vec![Row::new(
                ROW,
                Status::Fail,
                "Claude Code is not installed here: no `claude` on PATH",
            )];
        }
        Looked::Unreadable(why) => return vec![Row::new(ROW, Status::Fail, why)],
        Looked::Found(found) => found,
    };
    let claude = match (&found.wiring, &found.claude) {
        (Wiring::Wired { .. } | Wiring::Bridged(_), _) => {
            let row = wired(&found);
            return vec![Row::new(
                ROW,
                Status::Ok,
                format!("unchanged: {}", row.detail),
            )];
        }
        (Wiring::Twice(_) | Wiring::Taken(_), _) | (_, None) => return vec![wired(&found)],
        (Wiring::Broken(_) | Wiring::Unwired, Some(claude)) => claude,
    };
    let (program, on_path) = wiring::registered(this, found.path.as_deref());
    let aside = if on_path {
        String::new()
    } else {
        format!(
            " (this perspicax is not the one on PATH, so Claude Code runs {} itself)",
            program.display()
        )
    };
    let next = "; Claude Code starts it when it next starts";
    let row = |status, detail: String| vec![Row::new(ROW, status, detail)];
    match &found.wiring {
        Wiring::Broken(entry) => {
            let mut server = found.servers.get(&entry.name).cloned().unwrap_or_default();
            if let Some(server) = server.as_object_mut() {
                server.insert("command".to_owned(), json!(program));
                server.entry("type").or_insert_with(|| json!("stdio"));
            }
            let remove = ["mcp", "remove", "-s", "user", &entry.name];
            let added = server.to_string();
            let add = ["mcp", "add-json", "-s", "user", &entry.name, &added];
            let mend = format!(
                "`{}`, which runs {}: `{}`, then `{}`{aside}",
                entry.name,
                entry.command,
                shown(&remove),
                shown(&add)
            );
            if dry_run {
                return row(Status::Ok, format!("would mend {mend}"));
            }
            if let Err(why) = claude_mcp(claude, &remove) {
                return row(
                    Status::Fail,
                    format!("could not take out `{}` to mend it: {why}", entry.name),
                );
            }
            match claude_mcp(claude, &add) {
                Ok(()) => row(Status::Ok, format!("mended {mend}{next}")),
                Err(why) => row(
                    Status::Fail,
                    format!(
                        "took out `{}` to mend it, and could not put it back: {why}. Put it \
                         back with `{}`",
                        entry.name,
                        shown(&add)
                    ),
                ),
            }
        }
        _ => {
            let added =
                json!({ "type": "stdio", "command": program, "args": [ATTACH] }).to_string();
            let add = ["mcp", "add-json", "-s", "user", NAME, &added];
            if dry_run {
                return row(
                    Status::Ok,
                    format!("would add `{NAME}`: `{}`{aside}", shown(&add)),
                );
            }
            match claude_mcp(claude, &add) {
                Ok(()) => row(
                    Status::Ok,
                    format!("added `{NAME}`: `{}`{aside}{next}", shown(&add)),
                ),
                Err(why) => row(Status::Fail, format!("could not add `{NAME}`: {why}")),
            }
        }
    }
}

/// Take perspicax's servers out of Claude Code's user config: each that runs
/// `perspicax attach`, whatever it is called, working or not. A way in the
/// person made some other way is theirs, and stays. `dry_run` says what it
/// would run, and runs nothing.
pub fn uninstall(dry_run: bool, var: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    let found = match look(var) {
        Looked::Absent => {
            return vec![Row::new(
                ROW,
                Status::Skipped,
                "Claude Code is not installed here: nothing to take out",
            )];
        }
        Looked::Unreadable(why) => return vec![Row::new(ROW, Status::Fail, why)],
        Looked::Found(found) => found,
    };
    let entries = entries(&found.servers);
    let kinds: Vec<(&Entry, Kind)> = entries
        .iter()
        .map(|entry| (entry, Kind::of(entry, found.path.as_deref())))
        .collect();
    let mut rows: Vec<Row> = kinds
        .iter()
        .filter(|(_, kind)| *kind == Kind::Bridge)
        .map(|(entry, _)| {
            Row::new(
                ROW,
                Status::Ok,
                format!(
                    "kept `{}`, through `{}`: a way to the desktop that is yours",
                    entry.name,
                    entry.shown()
                ),
            )
        })
        .collect();
    let ours: Vec<&Entry> = kinds
        .iter()
        .filter(|(_, kind)| matches!(kind, Kind::Ours { .. }))
        .map(|(entry, _)| *entry)
        .collect();
    if ours.is_empty() {
        rows.insert(
            0,
            Row::new(ROW, Status::Ok, "nothing of perspicax's to take out"),
        );
        return rows;
    }
    let Some(claude) = &found.claude else {
        rows.insert(
            0,
            Row::new(
                ROW,
                Status::Fail,
                "there is no `claude` on PATH to take perspicax's server out with",
            ),
        );
        return rows;
    };
    for entry in ours {
        let remove = ["mcp", "remove", "-s", "user", &entry.name];
        let row = if dry_run {
            Row::new(
                ROW,
                Status::Ok,
                format!("would take out `{}`: `{}`", entry.name, shown(&remove)),
            )
        } else {
            match claude_mcp(claude, &remove) {
                Ok(()) => Row::new(
                    ROW,
                    Status::Ok,
                    format!("took out `{}`: `{}`", entry.name, shown(&remove)),
                ),
                Err(why) => Row::new(
                    ROW,
                    Status::Fail,
                    format!("could not take out `{}`: {why}", entry.name),
                ),
            }
        };
        rows.push(row);
    }
    rows
}

/// `claude ARGS`, as typed.
fn shown(args: &[&str]) -> String {
    wiring::shown(std::iter::once(CLAUDE).chain(args.iter().copied()))
}

/// Run `claude` with `args`, and say why not when it did not do it.
fn claude_mcp(claude: &Path, args: &[&str]) -> Result<(), String> {
    match wiring::ask(claude, args, &[]) {
        Ok(answer) if answer.status.success() => Ok(()),
        Ok(answer) => Err(answer.complaint()),
        Err(error) => Err(format!("{error:#}")),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A home folder for one test, with a `bin` on its `PATH`, emptied when it
    /// ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("perspicax-claude-{name}-{}", std::process::id()));
            fs::create_dir_all(dir.join("bin")).expect("a scratch folder");
            Self(dir)
        }

        /// Claude Code's config, holding `text`.
        fn config(&self, text: &str) {
            fs::write(self.0.join(".claude.json"), text).expect("a config");
        }

        /// A program at `name` under the folder, running `body`.
        fn program(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).expect("its folder");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a program");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("its mode");
            path
        }

        /// A stand-in `claude` on the `PATH`, writing each command line it is
        /// given to `claude.log`, then running `body`.
        fn claude(&self, body: &str) {
            let log = self.0.join("claude.log");
            self.program(
                "bin/claude",
                &format!("printf '%s\\n' \"$*\" >> '{}'\n{body}", log.display()),
            );
        }

        /// What the stand-in `claude` was asked, a line a command.
        fn asked(&self) -> Vec<String> {
            fs::read_to_string(self.0.join("claude.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// The environment a test runs in: this folder for its home, and its
        /// `bin` for its `PATH`.
        fn var(&self) -> impl Fn(&str) -> Option<String> + '_ {
            move |name| match name {
                "HOME" => Some(self.0.display().to_string()),
                "PATH" => Some(self.0.join("bin").display().to_string()),
                _ => None,
            }
        }

        /// This perspicax, where `cargo` would leave it.
        fn this(&self) -> PathBuf {
            self.program("target/debug/perspicax", "")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn one(rows: &[Row]) -> &Row {
        assert_eq!(rows.len(), 1, "{rows:?}");
        &rows[0]
    }

    #[test]
    fn claude_code_is_told_to_run_this_perspicax_attach() {
        let scratch = Scratch::new("adds");
        scratch.claude("exit 0");
        scratch.config(r#"{"mcpServers":{"github":{"type":"stdio","command":"npx"}}}"#);
        let this = scratch.this();
        let rows = install(&this, false, scratch.var());
        let row = one(&rows);
        assert_eq!(row.status, Status::Ok, "{row:?}");
        assert!(row.detail.starts_with("added `perspicax`"), "{row:?}");
        assert!(row.detail.contains("not the one on PATH"), "{row:?}");
        assert_eq!(
            scratch.asked(),
            [format!(
                r#"mcp add-json -s user perspicax {{"args":["attach"],"command":"{}","type":"stdio"}}"#,
                this.display()
            )]
        );
    }

    #[test]
    fn the_perspicax_on_path_is_the_one_registered_when_it_is_this_one() {
        let scratch = Scratch::new("onpath");
        scratch.claude("exit 0");
        let this = scratch.program("bin/perspicax", "");
        let row = install(&this, false, scratch.var()).remove(0);
        assert!(!row.detail.contains("not the one on PATH"), "{row:?}");
    }

    #[test]
    fn a_way_in_already_there_is_left_as_it_is() {
        let scratch = Scratch::new("unchanged");
        scratch.claude("exit 0");
        scratch.program("bin/perspicax", "");
        scratch.config(r#"{"mcpServers":{"perpicax":{"command":"perspicax","args":["attach"]}}}"#);
        let rows = install(&scratch.this(), false, scratch.var());
        let row = one(&rows);
        assert_eq!(row.status, Status::Ok);
        assert!(
            row.detail.starts_with("unchanged: wired as `perpicax`"),
            "{row:?}"
        );
        assert!(scratch.asked().is_empty(), "and nothing was run");
        assert_eq!(one(&status(scratch.var())).status, Status::Ok);
    }

    #[test]
    fn a_perspicax_that_is_gone_is_mended_keeping_the_rest_of_the_entry() {
        let scratch = Scratch::new("mends");
        scratch.claude("exit 0");
        scratch.config(
            r#"{"mcpServers":{"desktop":{"command":"/gone/perspicax","args":["attach","/run/x"],"env":{"A":"1"}}}}"#,
        );
        assert_eq!(one(&status(scratch.var())).status, Status::Warn);
        let this = scratch.this();
        let row = install(&this, false, scratch.var()).remove(0);
        assert_eq!(row.status, Status::Ok, "{row:?}");
        assert!(
            row.detail
                .starts_with("mended `desktop`, which runs /gone/perspicax"),
            "{row:?}"
        );
        assert_eq!(
            scratch.asked(),
            [
                "mcp remove -s user desktop".to_owned(),
                format!(
                    r#"mcp add-json -s user desktop {{"args":["attach","/run/x"],"command":"{}","env":{{"A":"1"}},"type":"stdio"}}"#,
                    this.display()
                )
            ]
        );
    }

    #[test]
    fn a_mend_that_fails_half_way_says_how_to_finish_it() {
        let scratch = Scratch::new("halfway");
        scratch.claude(
            "case \"$2\" in add-json) echo 'MCP server desktop already exists in user config' >&2; exit 1;; esac",
        );
        scratch.config(
            r#"{"mcpServers":{"desktop":{"command":"/gone/perspicax","args":["attach"]}}}"#,
        );
        let row = install(&scratch.this(), false, scratch.var()).remove(0);
        assert_eq!(row.status, Status::Fail);
        assert!(
            row.detail.starts_with(
                "took out `desktop` to mend it, and could not put it back: MCP server desktop \
                 already exists in user config. Put it back with `claude mcp add-json"
            ),
            "{row:?}"
        );
    }

    #[test]
    fn two_ways_in_or_a_name_taken_are_refused_untouched() {
        let scratch = Scratch::new("refused");
        scratch.claude("exit 0");
        scratch.config(
            r#"{"mcpServers":{"a":{"command":"/x/perspicax","args":["attach"]},"b":{"command":"nc","args":["-U","$PERSPICAX_MCP_SOCKET"]}}}"#,
        );
        let row = install(&scratch.this(), false, scratch.var()).remove(0);
        assert_eq!(row.status, Status::Fail);
        assert!(row.detail.contains("as `a` and `b`"), "{row:?}");
        scratch.config(
            r#"{"mcpServers":{"perspicax":{"command":"perspicax","args":["--headless","--mcp"]}}}"#,
        );
        let row = install(&scratch.this(), false, scratch.var()).remove(0);
        assert_eq!(row.status, Status::Fail);
        assert!(
            row.detail
                .starts_with("`perspicax` is `perspicax --headless --mcp`"),
            "{row:?}"
        );
        assert!(scratch.asked().is_empty(), "and nothing was run");
    }

    #[test]
    fn a_dry_run_runs_nothing() {
        let scratch = Scratch::new("dry");
        scratch.claude("exit 0");
        let row = install(&scratch.this(), true, scratch.var()).remove(0);
        assert!(
            row.detail
                .starts_with("would add `perspicax`: `claude mcp add-json"),
            "{row:?}"
        );
        assert!(scratch.asked().is_empty());
    }

    #[test]
    fn claude_codes_refusal_is_said_in_its_words() {
        let scratch = Scratch::new("refuses");
        scratch
            .claude("echo 'Cannot add MCP server: not allowed by enterprise policy' >&2; exit 1");
        let row = install(&scratch.this(), false, scratch.var()).remove(0);
        assert_eq!(row.status, Status::Fail);
        assert_eq!(
            row.detail,
            "could not add `perspicax`: Cannot add MCP server: not allowed by enterprise policy"
        );
    }

    #[test]
    fn without_claude_code_status_is_skipped_and_install_fails() {
        let scratch = Scratch::new("absent");
        assert_eq!(one(&status(scratch.var())).status, Status::Skipped);
        assert_eq!(
            one(&install(&scratch.this(), false, scratch.var())).status,
            Status::Fail
        );
        assert_eq!(
            one(&uninstall(false, scratch.var())).status,
            Status::Skipped
        );
    }

    #[test]
    fn a_config_claude_code_could_not_read_fails() {
        let scratch = Scratch::new("unreadable");
        scratch.claude("exit 0");
        scratch.config("{ not json");
        let row = one(&status(scratch.var())).clone();
        assert_eq!(row.status, Status::Fail);
        assert!(
            row.detail.contains("is not JSON Claude Code could read"),
            "{row:?}"
        );
        for text in ["", "  \n", "[1]", r#"{"mcpServers":3}"#] {
            scratch.config(text);
            let servers = read(&scratch.0.join(".claude.json")).unwrap().unwrap();
            assert!(servers.is_empty(), "{text:?}");
        }
    }

    #[test]
    fn only_a_server_claude_code_starts_as_a_program_could_be_perspicaxs() {
        let servers: Map<String, Value> = serde_json::from_str(
            r#"{"web":{"type":"http","url":"https://x"},"odd":{"command":3},"px":{"command":"perspicax","args":["attach",7]}}"#,
        )
        .unwrap();
        assert_eq!(
            entries(&servers),
            [Entry {
                name: "px".to_owned(),
                command: "perspicax".to_owned(),
                args: vec!["attach".to_owned()],
                enabled: true,
            }]
        );
    }

    #[test]
    fn uninstall_takes_out_perspicaxs_and_keeps_the_persons_own() {
        let scratch = Scratch::new("uninstall");
        scratch.claude("exit 0");
        scratch.config(
            r#"{"mcpServers":{"perpicax":{"command":"perspicax","args":["attach"]},"nc":{"command":"nc","args":["-U","$PERSPICAX_MCP_SOCKET"]}}}"#,
        );
        let rows = uninstall(false, scratch.var());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].detail.starts_with("kept `nc`"), "{rows:?}");
        assert!(
            rows[1].detail.starts_with("took out `perpicax`"),
            "{rows:?}"
        );
        assert_eq!(scratch.asked(), ["mcp remove -s user perpicax"]);
    }

    #[test]
    fn claude_codes_config_is_in_its_config_dir_or_home() {
        let var = |dir: Option<&'static str>, home: Option<&'static str>| {
            move |name: &str| match name {
                "CLAUDE_CONFIG_DIR" => dir.map(str::to_owned),
                "HOME" => home.map(str::to_owned),
                _ => None,
            }
        };
        assert_eq!(
            config_path(var(Some("/c"), Some("/h"))),
            Some(PathBuf::from("/c/.claude.json"))
        );
        assert_eq!(
            config_path(var(Some(""), Some("/h"))),
            Some(PathBuf::from("/h/.claude.json")),
            "an empty one is none"
        );
        assert_eq!(config_path(var(None, None)), None);
    }
}
