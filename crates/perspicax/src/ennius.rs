//! `perspicax ennius`: ennius, told how to reach the desktop.
//!
//! ennius starts an MCP server for a session whose grant names one of its
//! tools, `mcp:<server>:<tool>`, and denies a session every tool it was not
//! granted. So reaching the desktop takes two things: perspicax's server among
//! its `[tools.mcp]`, and perspicax's tools in the grant an interactive session
//! starts with, `[interactive.grant] tools`. `install` makes both so; `status`
//! says whether they are; `uninstall` takes out what is perspicax's, and leaves
//! a way in the person made some other way.
//!
//! Through ennius's own command line, `tools mcp` and `config grant`, and
//! never by editing its config: ennius refuses a file with a key it does not
//! know, and refuses it everywhere, the hooks Claude Code runs included. Every
//! question is asked with `--json`, so its answer is data and its refusal an
//! object on stderr; with the keyring left alone, since none of this needs a
//! credential and a locked keyring waits on its prompt with no end; and
//! without ennius's usage analytics, since perspicax asking is nobody using
//! ennius.
//!
//! One grant is never made: into ennius's built-in default. A grant is written
//! as the whole list, starting from what the file says, so into a file that
//! says nothing it would replace ennius's own tools rather than add to them.
//! Until the file lists them, that step is skipped, and says why.
//!
//! `attach` finds the session through `PERSPICAX_MCP_SOCKET`, so an ennius
//! started inside the session reaches it -- from a terminal, or the start
//! menu -- and one started outside it does not.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    report::{Row, Status},
    wiring::{self, ATTACH, Entry, Kind, NAME, Wiring},
};

/// ennius's command line.
const ENNIUS: &str = "ennius";

/// The command that puts this module's warnings right.
pub const INSTALL: &str = "perspicax ennius install";

/// Set for every ennius this asks: no keyring, no analytics.
const QUIET: [(&str, &str); 2] = [
    ("ENNIUS_DISABLE_KEYRING", "1"),
    ("ENNIUS_DISABLE_ANALYTICS", "1"),
];

/// What the server row is about.
const SERVER: &str = "server";

/// What the grant row is about.
const GRANT: &str = "grant";

/// What there is of ennius on this machine.
enum Looked {
    /// Nothing: no `ennius` on `PATH`.
    Absent,
    /// An ennius that would not answer, and its words.
    Unreadable(String),
    /// ennius, and what it says of perspicax.
    Found(Found),
}

struct Found {
    /// Its command line.
    ennius: PathBuf,
    /// Its servers.
    entries: Vec<Entry>,
    /// What those come to.
    wiring: Wiring,
    /// The tools an interactive session is granted, as the config file lists
    /// them; none when the file lists none and ennius's built-in default
    /// stands.
    granted: Option<Vec<String>>,
    /// Whether this ennius takes up an `mcp:` grant in every session: false
    /// for one from before it did (ennius #193), which listed no server's
    /// lifetime, as the change that brought it in began to. None with no
    /// server listed to tell by.
    current: Option<bool>,
    /// `PATH`, where a bare name is found.
    path: Option<OsString>,
}

fn look(var: impl Fn(&str) -> Option<String>) -> Looked {
    let path = var("PATH").map(OsString::from);
    let Some(ennius) = perspicax_config::which(ENNIUS, path.as_deref()) else {
        return Looked::Absent;
    };
    let answers = ask_json(&ennius, &["tools", "list"])
        .and_then(|listed| ask_json(&ennius, &["config", "show"]).map(|shown| (listed, shown)));
    let (listed, shown) = match answers {
        Ok(answers) => answers,
        Err(why) => return Looked::Unreadable(format!("ennius could not answer: {why}")),
    };
    let (entries, current) = servers(&listed);
    Looked::Found(Found {
        wiring: Wiring::of(&entries, path.as_deref()),
        granted: granted(&shown),
        ennius,
        entries,
        current,
        path,
    })
}

/// The servers `ennius --json tools list` lists, and whether it lists their
/// lifetimes: see [`Found::current`]. Read leniently, as ennius may say more
/// than this knows of.
fn servers(listed: &Value) -> (Vec<Entry>, Option<bool>) {
    let servers = listed
        .get("mcp_servers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let entries = servers
        .iter()
        .filter_map(|server| {
            Some(Entry {
                name: server.get("server")?.as_str()?.to_owned(),
                // A server the file names no program for runs none.
                command: server
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
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
                enabled: server
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .collect();
    let current = (!servers.is_empty()).then(|| {
        servers
            .iter()
            .any(|server| server.get("lifetime").is_some())
    });
    (entries, current)
}

/// The interactive grant's tools, from `ennius --json config show`: only as
/// the user's config file lists them, since that file is what a grant edits,
/// starting from what it says.
fn granted(shown: &Value) -> Option<Vec<String>> {
    let from = shown
        .get("provenance")
        .and_then(|provenance| provenance.get("interactive.grant.tools"))
        .and_then(Value::as_str);
    if from != Some("user") {
        return None;
    }
    let tools = shown
        .pointer("/config/interactive/grant/tools")?
        .as_array()?;
    Some(
        tools
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

/// Whether ennius reaches the desktop, as rows: its server, and its grant.
pub fn status(var: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    let looked = look(var);
    let found = match &looked {
        Looked::Absent => return absent(),
        Looked::Unreadable(why) => return vec![Row::new(SERVER, Status::Fail, why)],
        Looked::Found(found) => found,
    };
    let grant = match named(&found.wiring) {
        Some(name) => grant_said(found, name),
        None => Row::new(GRANT, Status::Skipped, "no server of perspicax's to grant"),
    };
    vec![server_said(found), grant]
}

/// The rows of a machine without ennius.
fn absent() -> Vec<Row> {
    vec![Row::new(
        SERVER,
        Status::Skipped,
        "ennius is not installed here: no `ennius` on PATH",
    )]
}

/// The name of the server that reaches the desktop, or will once it is
/// mended, if it is one name.
fn named(wiring: &Wiring) -> Option<&str> {
    match wiring {
        Wiring::Wired { name, .. } => Some(name),
        Wiring::Broken(entry) | Wiring::Bridged(entry) => Some(&entry.name),
        Wiring::Twice(_) | Wiring::Taken(_) | Wiring::Unwired => None,
    }
}

/// What `found` says of its server.
fn server_said(found: &Found) -> Row {
    let row = |status, detail: String| Row::new(SERVER, status, detail);
    let (status, detail) = match &found.wiring {
        Wiring::Wired {
            name,
            enabled: false,
            ..
        } => {
            return row(
                Status::Warn,
                format!("`{name}` is switched off: run `{INSTALL}`"),
            );
        }
        Wiring::Wired { name, runs, .. } => (
            Status::Ok,
            format!("wired as `{name}`, which runs {} {ATTACH}", runs.display()),
        ),
        Wiring::Bridged(entry) => (
            Status::Ok,
            format!(
                "ennius reaches the desktop as `{}`, through `{}`",
                entry.name,
                entry.shown()
            ),
        ),
        Wiring::Broken(entry) => {
            return row(
                Status::Warn,
                format!(
                    "`{}` runs {}, which is not there to run: run `{INSTALL}`",
                    entry.name, entry.command
                ),
            );
        }
        Wiring::Unwired => {
            return row(
                Status::Warn,
                format!("ennius has no way to the desktop: run `{INSTALL}`"),
            );
        }
        Wiring::Twice(names) => return row(Status::Fail, twice(names)),
        Wiring::Taken(entry) => {
            return row(
                Status::Fail,
                format!(
                    "`{NAME}` is `{}`, which does not reach the desktop: rename or remove it, \
                     then run `{INSTALL}`",
                    entry.shown()
                ),
            );
        }
    };
    if found.current == Some(false) {
        return row(
            Status::Warn,
            format!(
                "{detail}, but this ennius takes up no `mcp:` grant in a chat, the TUI or the \
                 GUI, only in `agent run`: upgrade ennius past its #193"
            ),
        );
    }
    row(status, detail)
}

/// Why more than one way to the desktop is left as it is.
fn twice(names: &[String]) -> String {
    let named = names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        "ennius reaches the desktop as {named}, and the desktop holds one conversation at a \
         time: keep one, and take out the others with `ennius tools mcp remove NAME`"
    )
}

/// perspicax's tools, as ennius grants them under the server `name`.
fn wanted(name: &str) -> Vec<String> {
    perspicax_mcp::Perspicax::tool_names()
        .iter()
        .map(|tool| format!("mcp:{name}:{tool}"))
        .collect()
}

/// Why no grant is made into ennius's built-in default.
const BUILT_IN: &str = "ennius's interactive sessions start with its built-in tools, which a \
                        grant would replace rather than add to: list them under \
                        `[interactive.grant] tools` in its config first";

/// What `found` says of the grant to `name`.
fn grant_said(found: &Found, name: &str) -> Row {
    let Some(granted) = &found.granted else {
        return Row::new(GRANT, Status::Warn, BUILT_IN);
    };
    let wanted = wanted(name);
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|tool| !granted.contains(tool))
        .map(|tool| tool.rsplit(':').next().unwrap_or(tool))
        .collect();
    if missing.is_empty() {
        return Row::new(
            GRANT,
            Status::Ok,
            format!(
                "all {} of perspicax's tools are granted, as `mcp:{name}:*`",
                wanted.len()
            ),
        );
    }
    Row::new(
        GRANT,
        Status::Warn,
        format!(
            "{} of {} granted; missing {}: run `{INSTALL}`",
            wanted.len() - missing.len(),
            wanted.len(),
            missing.join(", ")
        ),
    )
}

/// Put perspicax's server in ennius's config, or mend the one there, so that
/// it runs `this` -- or the perspicax on `PATH` that is `this`,
/// [`wiring::registered`] -- with `attach`; then grant an interactive session
/// every tool perspicax serves, under that server's name. Leaves a way in
/// that works as it is, and refuses to choose between two, or to take a name
/// somebody else's server holds. `dry_run` says what it would run, and runs
/// nothing.
pub fn install(this: &Path, dry_run: bool, var: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    let found = match look(var) {
        Looked::Absent => return absent(),
        Looked::Unreadable(why) => return vec![Row::new(SERVER, Status::Fail, why)],
        Looked::Found(found) => found,
    };
    let (program, on_path) = wiring::registered(this, found.path.as_deref());
    let aside = if on_path {
        String::new()
    } else {
        format!(
            " (this perspicax is not the one on PATH, so ennius runs {} itself)",
            program.display()
        )
    };
    let shown_program = program.display().to_string();
    let change: Option<(&str, Vec<&str>)> = match &found.wiring {
        Wiring::Unwired => Some((
            "add",
            vec!["tools", "mcp", "add", NAME, "--", &shown_program, ATTACH],
        )),
        Wiring::Broken(entry) => Some((
            "mend",
            vec![
                "tools",
                "mcp",
                "set",
                &entry.name,
                "--command",
                &shown_program,
            ],
        )),
        Wiring::Wired {
            name,
            enabled: false,
            ..
        } => Some(("switch on", vec!["tools", "mcp", "set", name, "--enable"])),
        Wiring::Wired { .. } | Wiring::Bridged(_) => None,
        Wiring::Twice(_) | Wiring::Taken(_) => {
            return vec![
                server_said(&found),
                Row::new(GRANT, Status::Skipped, "no server of perspicax's to grant"),
            ];
        }
    };
    let name = named(&found.wiring).unwrap_or(NAME);
    let server = match change {
        None => {
            let said = server_said(&found);
            Row::new(SERVER, said.status, format!("unchanged: {}", said.detail))
        }
        Some((verb, args)) if dry_run => Row::new(
            SERVER,
            Status::Ok,
            format!("would {verb} `{name}`: `{}`{aside}", shown(&args)),
        ),
        Some((verb, args)) => match ennius_do(&found.ennius, &args) {
            Ok(()) => Row::new(
                SERVER,
                Status::Ok,
                format!("{} `{name}`: `{}`{aside}", done(verb), shown(&args)),
            ),
            Err(why) => {
                return vec![
                    Row::new(
                        SERVER,
                        Status::Fail,
                        format!("could not {verb} `{name}`: {why}"),
                    ),
                    Row::new(GRANT, Status::Skipped, "no server of perspicax's to grant"),
                ];
            }
        },
    };
    vec![server, grant(&found, name, dry_run)]
}

/// The past of what was done.
fn done(verb: &str) -> &'static str {
    match verb {
        "add" => "added",
        "mend" => "mended",
        _ => "switched on",
    }
}

/// Grant an interactive session the tools of perspicax's that it lacks, under
/// the server `name`.
fn grant(found: &Found, name: &str, dry_run: bool) -> Row {
    let Some(granted) = &found.granted else {
        return Row::new(GRANT, Status::Skipped, format!("not granted: {BUILT_IN}"));
    };
    let missing: Vec<String> = wanted(name)
        .into_iter()
        .filter(|tool| !granted.contains(tool))
        .collect();
    if missing.is_empty() {
        return Row::new(
            GRANT,
            Status::Ok,
            format!("unchanged: every tool of perspicax's is granted, as `mcp:{name}:*`"),
        );
    }
    let mut args = vec!["config", "grant", "add"];
    for tool in &missing {
        args.extend(["--tool", tool]);
    }
    let what = format!(
        "{} tools as `mcp:{name}:*`: `{}`",
        missing.len(),
        shown(&args)
    );
    if dry_run {
        return Row::new(GRANT, Status::Ok, format!("would grant {what}"));
    }
    match ennius_do(&found.ennius, &args) {
        Ok(()) => Row::new(GRANT, Status::Ok, format!("granted {what}")),
        Err(why) => Row::new(GRANT, Status::Fail, format!("could not grant them: {why}")),
    }
}

/// Take perspicax's servers out of ennius's config, each that runs `perspicax
/// attach` whatever it is called, with what is granted under their names. A
/// way in the person made some other way is theirs, and stays. `dry_run` says
/// what it would run, and runs nothing.
pub fn uninstall(dry_run: bool, var: impl Fn(&str) -> Option<String>) -> Vec<Row> {
    let found = match look(var) {
        Looked::Absent => return absent(),
        Looked::Unreadable(why) => return vec![Row::new(SERVER, Status::Fail, why)],
        Looked::Found(found) => found,
    };
    let mut rows = Vec::new();
    let mut ours = Vec::new();
    for entry in &found.entries {
        match Kind::of(entry, found.path.as_deref()) {
            Kind::Ours { .. } => ours.push(entry),
            Kind::Bridge => rows.push(Row::new(
                SERVER,
                Status::Ok,
                format!(
                    "kept `{}`, through `{}`: a way to the desktop that is yours",
                    entry.name,
                    entry.shown()
                ),
            )),
            Kind::Other => {}
        }
    }
    if ours.is_empty() {
        rows.insert(
            0,
            Row::new(SERVER, Status::Ok, "nothing of perspicax's to take out"),
        );
        return rows;
    }
    for entry in ours {
        // The grant first: a grant is to a name, and outlives the server.
        // Never asked to remove nothing, which into a file that lists no
        // tools would write it an empty list, and take ennius's own away.
        let prefix = format!("mcp:{}:", entry.name);
        let granted: Vec<&String> = found
            .granted
            .iter()
            .flatten()
            .filter(|tool| tool.starts_with(&prefix))
            .collect();
        if !granted.is_empty() {
            let mut args = vec!["config", "grant", "remove"];
            for tool in &granted {
                args.extend(["--tool", tool.as_str()]);
            }
            rows.push(run(
                &found.ennius,
                &args,
                dry_run,
                GRANT,
                &format!("{} tools granted as `{prefix}*`", granted.len()),
            ));
        }
        let args = ["tools", "mcp", "remove", &entry.name];
        rows.push(run(
            &found.ennius,
            &args,
            dry_run,
            SERVER,
            &format!("`{}`", entry.name),
        ));
    }
    rows
}

/// Take out `what` with `args`, or say that it would.
fn run(ennius: &Path, args: &[&str], dry_run: bool, row: &'static str, what: &str) -> Row {
    let command = shown(args);
    if dry_run {
        return Row::new(
            row,
            Status::Ok,
            format!("would take out {what}: `{command}`"),
        );
    }
    match ennius_do(ennius, args) {
        Ok(()) => Row::new(row, Status::Ok, format!("took out {what}: `{command}`")),
        Err(why) => Row::new(
            row,
            Status::Fail,
            format!("could not take out {what}: {why}"),
        ),
    }
}

/// `ennius ARGS`, as typed.
fn shown(args: &[&str]) -> String {
    wiring::shown(std::iter::once(ENNIUS).chain(args.iter().copied()))
}

/// Ask ennius `args`, and read its answer.
fn ask_json(ennius: &Path, args: &[&str]) -> Result<Value, String> {
    let answer = asked(ennius, args)?;
    serde_json::from_str(&answer).map_err(|error| format!("its answer was not JSON: {error}"))
}

/// Have ennius do `args`.
fn ennius_do(ennius: &Path, args: &[&str]) -> Result<(), String> {
    asked(ennius, args).map(drop)
}

/// What ennius said on stdout to `--json` and `args`, or its refusal.
fn asked(ennius: &Path, args: &[&str]) -> Result<String, String> {
    let args: Vec<&str> = std::iter::once("--json")
        .chain(args.iter().copied())
        .collect();
    let answer = wiring::ask(ennius, &args, &QUIET).map_err(|error| format!("{error:#}"))?;
    if answer.status.success() {
        return Ok(answer.stdout);
    }
    // Under `--json`, its refusal is an object on stderr.
    let refusal = serde_json::from_str::<Value>(&answer.stderr)
        .ok()
        .and_then(|said| said.pointer("/error/message")?.as_str().map(str::to_owned));
    Err(refusal.unwrap_or_else(|| answer.complaint()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A folder for one test, with a `bin` on its `PATH`, emptied when it ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("perspicax-ennius-{name}-{}", std::process::id()));
            fs::create_dir_all(dir.join("bin")).expect("a scratch folder");
            Self(dir)
        }

        /// A program at `name` under the folder, running `body`: written
        /// as [`crate::forked`] says.
        fn program(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            crate::forked::program(&path, &format!("#!/bin/sh\n{body}\n"));
            path
        }

        /// A stand-in `ennius` on the `PATH`, writing each command line it is
        /// given to `ennius.log`; answering `tools list` with `servers` and
        /// `config show` with a grant of `tools` from the file (none: from
        /// ennius's default); and running `body` for anything else.
        fn ennius(&self, servers: &str, tools: Option<&str>, body: &str) {
            let log = self.0.join("ennius.log");
            let show = match tools {
                Some(tools) => format!(
                    r#"{{"config":{{"interactive":{{"grant":{{"tools":{tools}}}}}}},"provenance":{{"interactive.grant.tools":"user"}}}}"#
                ),
                None => r#"{"config":{"interactive":{"grant":{"tools":null}}},"provenance":{"interactive.grant.tools":"default"}}"#.to_owned(),
            };
            self.program(
                "bin/ennius",
                &format!(
                    "case \"$*\" in\n\
                     '--json tools list') printf '%s\\n' '{{\"tools\":[],\"mcp_servers\":{servers}}}' ;;\n\
                     '--json config show') printf '%s\\n' '{show}' ;;\n\
                     *) printf '%s\\n' \"$*\" >> '{}'\n{body} ;;\n\
                     esac",
                    log.display()
                ),
            );
        }

        /// What the stand-in was asked to change, a line a command.
        fn asked(&self) -> Vec<String> {
            fs::read_to_string(self.0.join("ennius.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        fn var(&self) -> impl Fn(&str) -> Option<String> + '_ {
            move |name| (name == "PATH").then(|| self.0.join("bin").display().to_string())
        }

        fn this(&self) -> PathBuf {
            self.program("target/debug/perspicax", "")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// The `--tool` flags granting perspicax's tools under `name`.
    fn tools(verb: &str, name: &str) -> String {
        let flags: Vec<String> = perspicax_mcp::Perspicax::tool_names()
            .iter()
            .map(|tool| format!("--tool mcp:{name}:{tool}"))
            .collect();
        format!("--json config grant {verb} {}", flags.join(" "))
    }

    /// dave's ennius, before any install: a server of his own naming, from an
    /// ennius that lists no lifetimes, and nothing of it granted.
    const DAVES: &str =
        r#"[{"server":"perpicax","command":"perspicax","args":["attach"],"enabled":true}]"#;

    #[test]
    fn an_ennius_with_no_way_in_is_given_one_and_granted_its_tools() {
        let scratch = Scratch::new("adds");
        scratch.ennius("[]", Some(r#"["read_file"]"#), "");
        let this = scratch.this();
        let rows = install(&this, false, scratch.var());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(
            (rows[0].status, rows[1].status),
            (Status::Ok, Status::Ok),
            "{rows:?}"
        );
        assert!(rows[0].detail.starts_with("added `perspicax`"), "{rows:?}");
        assert!(
            rows[1]
                .detail
                .starts_with("granted 11 tools as `mcp:perspicax:*`"),
            "{rows:?}"
        );
        assert_eq!(
            scratch.asked(),
            [
                format!(
                    "--json tools mcp add perspicax -- {} attach",
                    this.display()
                ),
                tools("add", "perspicax"),
            ]
        );
    }

    #[test]
    fn daves_own_server_is_kept_and_granted_under_its_name() {
        let scratch = Scratch::new("daves");
        scratch.program("bin/perspicax", "");
        scratch.ennius(DAVES, Some(r#"["read_file","echo"]"#), "");
        let rows = status(scratch.var());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].status, Status::Warn, "{rows:?}");
        assert!(
            rows[0].detail.contains("upgrade ennius past its #193"),
            "{rows:?}"
        );
        assert_eq!(rows[1].status, Status::Warn, "{rows:?}");
        assert!(
            rows[1]
                .detail
                .starts_with("0 of 11 granted; missing act, deltas"),
            "{rows:?}"
        );
        let rows = install(&scratch.this(), false, scratch.var());
        assert!(
            rows[0].detail.starts_with("unchanged: wired as `perpicax`"),
            "{rows:?}"
        );
        assert_eq!(scratch.asked(), [tools("add", "perpicax")]);
    }

    #[test]
    fn a_grant_is_not_written_into_enniuss_built_in_default() {
        let scratch = Scratch::new("default");
        scratch.ennius("[]", None, "");
        let rows = install(&scratch.this(), false, scratch.var());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[1].status, Status::Skipped, "{rows:?}");
        assert!(
            rows[1]
                .detail
                .contains("list them under `[interactive.grant] tools`"),
            "{rows:?}"
        );
        assert_eq!(
            scratch.asked().len(),
            1,
            "the server only: {:?}",
            scratch.asked()
        );
        assert_eq!(
            status(scratch.var())[1].status,
            Status::Skipped,
            "no server to grant yet"
        );
    }

    #[test]
    fn a_perspicax_that_is_gone_is_mended_and_one_switched_off_switched_on() {
        let scratch = Scratch::new("mends");
        scratch.ennius(
            r#"[{"server":"desktop","command":"/gone/perspicax","args":["attach","/run/x"],"enabled":true,"lifetime":"ephemeral"}]"#,
            Some("[]"),
            "",
        );
        let this = scratch.this();
        let rows = install(&this, false, scratch.var());
        assert!(rows[0].detail.starts_with("mended `desktop`"), "{rows:?}");
        assert_eq!(
            scratch.asked()[0],
            format!("--json tools mcp set desktop --command {}", this.display()),
            "the command alone, so the socket it names stays"
        );

        let scratch = Scratch::new("switched");
        scratch.program("bin/perspicax", "");
        scratch.ennius(
            r#"[{"server":"perspicax","command":"perspicax","args":["attach"],"enabled":false,"lifetime":"ephemeral"}]"#,
            Some("[]"),
            "",
        );
        assert_eq!(status(scratch.var())[0].status, Status::Warn);
        let rows = install(&scratch.this(), false, scratch.var());
        assert!(
            rows[0].detail.starts_with("switched on `perspicax`"),
            "{rows:?}"
        );
        assert_eq!(
            scratch.asked()[0],
            "--json tools mcp set perspicax --enable"
        );
    }

    #[test]
    fn two_ways_in_are_refused_and_nothing_is_granted() {
        let scratch = Scratch::new("twice");
        scratch.ennius(
            r#"[{"server":"a","command":"/x/perspicax","args":["attach"]},{"server":"b","command":"socat","args":["STDIO","UNIX-CONNECT:/run/user/1000/perspicax-mcp"]}]"#,
            Some("[]"),
            "",
        );
        let rows = install(&scratch.this(), false, scratch.var());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].status, Status::Fail, "{rows:?}");
        assert!(rows[0].detail.contains("as `a` and `b`"), "{rows:?}");
        assert_eq!(rows[1].status, Status::Skipped);
        assert!(scratch.asked().is_empty());
    }

    #[test]
    fn enniuss_refusal_is_said_in_its_words() {
        let scratch = Scratch::new("refuses");
        scratch.ennius(
            "[]",
            Some("[]"),
            r#"printf '%s\n' '{"error":{"code":"usage","exit_code":2,"message":"`perspicax` is already a server"}}' >&2; exit 2"#,
        );
        let rows = install(&scratch.this(), false, scratch.var());
        assert_eq!(rows[0].status, Status::Fail);
        assert_eq!(
            rows[0].detail,
            "could not add `perspicax`: `perspicax` is already a server"
        );
    }

    #[test]
    fn an_ennius_that_will_not_answer_fails_and_none_at_all_is_skipped() {
        let scratch = Scratch::new("unanswering");
        assert_eq!(status(scratch.var())[0].status, Status::Skipped);
        scratch.program(
            "bin/ennius",
            r#"printf '%s\n' '{"error":{"code":"config","exit_code":78,"message":"config: unknown field `tool`"}}' >&2; exit 78"#,
        );
        let rows = status(scratch.var());
        assert_eq!(rows[0].status, Status::Fail);
        assert_eq!(
            rows[0].detail,
            "ennius could not answer: config: unknown field `tool`"
        );
    }

    #[test]
    fn a_dry_run_runs_nothing() {
        let scratch = Scratch::new("dry");
        scratch.ennius("[]", Some("[]"), "");
        let rows = install(&scratch.this(), true, scratch.var());
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows[0].detail.starts_with("would add `perspicax`"),
            "{rows:?}"
        );
        assert!(
            rows[1].detail.starts_with("would grant 11 tools"),
            "{rows:?}"
        );
        assert!(scratch.asked().is_empty());
    }

    #[test]
    fn uninstall_takes_the_grant_then_the_server_and_keeps_the_rest() {
        let scratch = Scratch::new("uninstall");
        scratch.ennius(
            r#"[{"server":"perpicax","command":"perspicax","args":["attach"]},{"server":"nc","command":"nc","args":["-U","$PERSPICAX_MCP_SOCKET"]}]"#,
            Some(r#"["read_file","mcp:perpicax:act","mcp:perpicax:gone_tool","mcp:nc:act"]"#),
            "",
        );
        let rows = uninstall(false, scratch.var());
        assert!(rows[0].detail.starts_with("kept `nc`"), "{rows:?}");
        assert_eq!(
            scratch.asked(),
            [
                "--json config grant remove --tool mcp:perpicax:act --tool mcp:perpicax:gone_tool",
                "--json tools mcp remove perpicax"
            ]
        );
    }

    #[test]
    fn uninstall_never_asks_to_remove_a_grant_from_enniuss_default() {
        let scratch = Scratch::new("default-uninstall");
        scratch.ennius(DAVES, None, "");
        uninstall(false, scratch.var());
        assert_eq!(scratch.asked(), ["--json tools mcp remove perpicax"]);
    }

    #[test]
    fn the_grant_is_read_only_from_the_file() {
        let shown = |from: &str| -> Value {
            serde_json::from_str(&format!(
                r#"{{"config":{{"interactive":{{"grant":{{"tools":["x"]}}}}}},"provenance":{{"interactive.grant.tools":"{from}"}}}}"#
            ))
            .unwrap()
        };
        assert_eq!(granted(&shown("user")), Some(vec!["x".to_owned()]));
        assert_eq!(granted(&shown("default")), None);
        assert_eq!(
            granted(&shown("env")),
            None,
            "a grant would not edit what said it"
        );
    }
}
