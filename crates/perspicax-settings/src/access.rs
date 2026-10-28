//! Who may reach past their own windows, and what an agent may do: the
//! `[protocols]` and `[agent]` tables, read to be shown and never written.
//!
//! These rules are the file's boundary. The app shows each one in force
//! beside its profile's, and leaves changing them to an editor, so that
//! nothing which drives the app can widen what the file allows.

use perspicax_config::{AgentSocket, Config, SETTINGS_APP};
use perspicax_policy::Never;
use perspicax_policy::{Program, Protocol, Rule};

use crate::edit::EVERYTHING;

/// One rule: its key, what it decides, the rule in force, the profile's,
/// and whether the file writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The table it is written in: `protocols`, or `agent`.
    pub table: &'static str,
    pub key: &'static str,
    /// What it decides, in a line.
    pub about: &'static str,
    pub in_force: Rule,
    pub profile: Rule,
    pub written: bool,
}

/// Every `[protocols]` rule, in the file's order, then `[agent] launch`, as
/// `text` writes them and `config`, read from it, has them in force.
#[must_use]
pub fn grants(text: &str, config: &Config) -> Vec<Grant> {
    let profile = Config::profile(config.profile, EVERYTHING);
    let root = text.parse::<toml::Table>().unwrap_or_default();
    let written = |table: &str, key: &str| {
        root.get(table)
            .and_then(toml::Value::as_table)
            .is_some_and(|table| table.contains_key(key))
    };
    Protocol::ALL
        .into_iter()
        .map(|protocol| Grant {
            table: "protocols",
            key: protocol.key(),
            about: about(protocol),
            in_force: config.protocols.rule(protocol).clone(),
            profile: profile.protocols.rule(protocol).clone(),
            written: written("protocols", protocol.key()),
        })
        .chain(std::iter::once(Grant {
            table: "agent",
            key: "launch",
            about: "what an agent may start for itself, and act on",
            in_force: config.agent.launch.clone(),
            profile: profile.agent.launch.clone(),
            written: written("agent", "launch"),
        }))
        .collect()
}

/// What `protocol` lets a program do, in a line.
fn about(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::ForeignToplevelList => "list every window's title and app",
        Protocol::ForeignToplevelManagement => {
            "list every window, and raise, minimize or close it: a taskbar"
        }
        Protocol::Workspace => "list the workspaces, and switch between them: a pager",
        Protocol::Screencopy => "read every pixel on a monitor: a screenshot or recording",
        Protocol::OutputManagement => "list the monitors, and change their modes and places",
        Protocol::Shell => "speak for the desktop shell: its menus, and ending the session",
        Protocol::Settings => "hear the next chord pressed, before any binding: this app",
        Protocol::SessionLock => "lock the screen, and be typed the password that unlocks it",
    }
}

/// A rule in words: who it admits.
#[must_use]
pub fn rule_words(rule: &Rule) -> String {
    match rule {
        Rule::Off => "none: not offered".to_owned(),
        Rule::Any => "any program".to_owned(),
        Rule::Only(programs) => programs
            .iter()
            .map(|program| match program {
                Program::Installed(name) => format!("the system's {name}"),
                other => other.spelled().to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// The programs no agent is ever given, in words: the person's list, then
/// the settings app, which no list takes away.
#[must_use]
pub fn never_words(never: &Never) -> String {
    never
        .named()
        .into_iter()
        .chain([SETTINGS_APP])
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where the session serves an agent inside it, in words.
#[must_use]
pub fn socket_words(socket: &AgentSocket) -> String {
    match socket {
        AgentSocket::Unsaid => "as `--mcp-socket` says; none without it".to_owned(),
        AgentSocket::Off => "none, whatever `--mcp-socket` says".to_owned(),
        AgentSocket::On => {
            "perspicax-mcp in $XDG_RUNTIME_DIR, or where `--mcp-socket` says".to_owned()
        }
        AgentSocket::At(path) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use perspicax_config::Profile;
    use perspicax_policy::Program;

    use super::*;

    #[test]
    fn what_no_agent_is_given_ends_with_the_settings_app() {
        let never = |text: &str| {
            never_words(
                &perspicax_config::parse(text, EVERYTHING)
                    .expect("a config")
                    .agent
                    .never,
            )
        };
        assert_eq!(never(""), "ennius, perspicax-settings");
        assert_eq!(never("[agent]\nnever = []"), "perspicax-settings");
        assert_eq!(
            never("[agent]\nnever = [\"ennius\", \"/opt/wallet/bin/wallet\"]"),
            "ennius, /opt/wallet/bin/wallet, perspicax-settings"
        );
    }

    fn read(text: &str) -> Vec<Grant> {
        let config = perspicax_config::parse(text, EVERYTHING).expect("a config");
        grants(text, &config)
    }

    #[test]
    fn every_protocol_has_a_row_in_the_files_order() {
        let keys: Vec<&str> = read("").iter().map(|grant| grant.key).collect();
        let mut expected: Vec<&str> = Protocol::ALL.iter().map(|p| p.key()).collect();
        expected.push("launch");
        assert_eq!(keys, expected);
    }

    #[test]
    fn a_written_rule_is_told_apart_from_the_profiles() {
        let grants = read("[protocols]\nscreencopy = [\"/usr/bin/grim\"]\nworkspace = \"off\"\n");
        let screencopy = grants.iter().find(|g| g.key == "screencopy").unwrap();
        assert!(screencopy.written);
        assert_eq!(
            screencopy.in_force,
            Rule::Only(vec![Program::Path("/usr/bin/grim".to_owned())])
        );
        assert_eq!(
            rule_words(&screencopy.profile),
            "grim, wf-recorder, xdg-desktop-portal-wlr"
        );
        let workspace = grants.iter().find(|g| g.key == "workspace").unwrap();
        assert_eq!((workspace.written, &workspace.in_force), (true, &Rule::Off));
        assert_eq!(workspace.profile, Rule::Any);
        let shell = grants.iter().find(|g| g.key == "shell").unwrap();
        assert!(!shell.written);
        assert_eq!(shell.in_force, shell.profile);
    }

    #[test]
    fn a_rule_reads_in_words() {
        assert_eq!(rule_words(&Rule::Off), "none: not offered");
        assert_eq!(rule_words(&Rule::Any), "any program");
        let only = Rule::Only(vec![
            Program::parse("kanshi"),
            Program::parse("/usr/bin/wlr-randr"),
        ]);
        assert_eq!(rule_words(&only), "kanshi, /usr/bin/wlr-randr");
        let lock = Rule::Only(vec![
            Program::parse("swaylock").installed(),
            Program::parse("/opt/lock/bin/lock"),
        ]);
        assert_eq!(
            rule_words(&lock),
            "the system's swaylock, /opt/lock/bin/lock"
        );
    }

    #[test]
    fn the_agents_launch_rule_is_off_in_both_profiles() {
        for profile in Profile::ALL {
            let text = format!("profile = \"{}\"\n", profile.key());
            let launch = read(&text).into_iter().last().unwrap();
            assert_eq!((launch.table, launch.key), ("agent", "launch"));
            assert_eq!((launch.in_force, launch.profile), (Rule::Off, Rule::Off));
        }
        let launch = read("[agent]\nlaunch = [\"flutter\"]\n")
            .into_iter()
            .last()
            .unwrap();
        assert!(launch.written);
        assert_eq!(rule_words(&launch.in_force), "flutter");
    }
}
