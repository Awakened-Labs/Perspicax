//! What an agent is granted where a person has to agree to it, and how long
//! a grant lasts: on a headless desk that consents as a seat does.
//!
//! A grant names a pid, and the kernel gives an exited process's pid to the
//! next program that asks once the exited one is reaped. So every grant must
//! end with its process, or a program nobody granted could arrive holding
//! one. A reused pid can't be arranged on demand, so what is tested is the
//! condition that makes reuse harmless: a grant whose process has exited is
//! no longer in the consent the compositor publishes.
//!
//! Like the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::{
    path::Path,
    process::{Child, Command, Stdio},
};

use common::{Renamed, Session, until};
use perspicax_compositor::{Backend, Host, LaunchError};
use perspicax_index::{Consent, HostFacts, Refusal, SurfaceFacts, judge};
use perspicax_node::{Rect, Visibility};
use perspicax_policy::{Action, Never, Program, Rule};

/// Set, to the compositor's socket, for the copy of this binary that
/// [`window_probe`] runs in.
const PROBE: &str = "PERSPICAX_WINDOW_PROBE";

/// A desk that consents as a seat does.
fn granting(name: &str) -> Session {
    Session::start(name, Backend::headless((1280, 1024)).granting())
}

fn titled<'a>(facts: &'a HostFacts, title: &str) -> Option<&'a SurfaceFacts> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some(title))
}

/// A window titled "probe", put up by a process of its own: a copy of this
/// binary running [`window_probe`]. It stays until it is killed.
fn probe(session: &Session) -> Child {
    probe_as(
        session,
        &std::env::current_exe().expect("this test's binary"),
    )
}

/// The same, put up by `program`, this binary under another name.
fn probe_as(session: &Session, program: &Path) -> Child {
    let child = Command::new(program)
        .args(["--exact", "window_probe", "--ignored", "--test-threads=1"])
        .env(PROBE, session.socket())
        .stdout(Stdio::null())
        .spawn()
        .expect("the probe starts");
    session.wait_for(|facts| titled(facts, "probe").is_some_and(|window| window.mapped));
    child
}

/// The pids granted, once the compositor has published any consent. Before
/// its first snapshot a reader sees the default, nobody.
fn granted(facts: &HostFacts) -> Option<&[u32]> {
    match facts.consent() {
        Consent::Granted { pids, .. } => Some(pids),
        Consent::Nobody => None,
        Consent::Everyone => panic!("consent is not by grant"),
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_spawned_programs_grant_ends_when_it_exits() {
    let session = Session::start_spawning(
        "grant-ends",
        Backend::headless((800, 600)).granting(),
        vec![vec!["sleep".to_owned(), "1".to_owned()]],
    );
    let facts = session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.len() == 1));
    let pid = granted(&facts).expect("published")[0];
    assert!(
        std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "granted while it runs"
    );

    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop(());
}

/// The person's key on the focused window: its process granted, and taken
/// back by the same key.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_key_grants_the_focused_windows_process_and_takes_it_back() {
    let session = granting("grant-key");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "mine", "mine");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.wait_for(|facts| titled(facts, "mine").is_some_and(|window| window.mapped));
    let me = std::process::id();

    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts) == Some(&[me][..]));

    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop((desk, queue));
}

/// `revoke-agent-all` takes back a key's grant and `--spawn`'s alike.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn revoking_every_grant_leaves_nothing_consented() {
    let session = Session::start_spawning(
        "grant-revoke-all",
        Backend::headless((1280, 1024)).granting(),
        vec![vec!["sleep".to_owned(), "30".to_owned()]],
    );
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "mine", "mine");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    session.wait_for(|facts| titled(facts, "mine").is_some_and(|window| window.mapped));
    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.len() == 2));

    session.perform(Action::RevokeAgents);
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop((desk, queue));
}

/// A process this compositor did not start is not reaped by it, so nothing
/// here would notice it exit but its pidfd. The grant goes when the process
/// does, before anyone reaps it: before its pid could be anyone else's.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_in_a_process_of_its_own_loses_its_grant_when_the_process_exits() {
    let session = granting("grant-exits");
    let mut probe = probe(&session);
    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts) == Some(&[probe.id()][..]));

    probe.kill().expect("the probe is ours to kill");
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    probe
        .wait()
        .expect("and ours to reap, after its grant has gone");
    session.stop(());
}

/// A binding the person marked `agent = true`: what it starts is granted,
/// until it exits. Anywhere, as `--spawn`'s programs are.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_binding_that_spawns_for_the_agent_grants_what_it_starts_until_it_exits() {
    let session = granting("grant-binding");
    session.perform(Action::Spawn {
        command: vec!["sleep".to_owned(), "1".to_owned()],
        agent: true,
    });
    session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.len() == 1));
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop(());
}

/// Without `agent`, a binding's program is the person's. Headless starts
/// none at all, having no person; what is checked is that one marked for
/// the agent after it is the only thing granted.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_binding_spawn_without_agent_grants_nothing() {
    let session = granting("grant-binding-person");
    let sleep = vec!["sleep".to_owned(), "30".to_owned()];
    session.perform(Action::Spawn {
        command: sleep.clone(),
        agent: false,
    });
    session.perform(Action::Spawn {
        command: sleep,
        agent: true,
    });
    let facts = session.wait_for(|facts| granted(facts).is_some_and(|pids| !pids.is_empty()));
    assert_eq!(
        granted(&facts).map(<[u32]>::len),
        Some(1),
        "the agent's alone"
    );
    session.stop(());
}

/// A desk that consents as a seat does, where an agent may start only the
/// programs `named`, as `[agent] launch` would name them.
fn launching(name: &str, named: &[&str]) -> Session {
    let rule = Rule::Only(
        named
            .iter()
            .map(|program| Program::parse(program))
            .collect(),
    );
    Session::start(
        name,
        Backend::headless((800, 600)).granting().launching(rule),
    )
}

fn words(command: &[&str]) -> Vec<String> {
    command.iter().map(|&word| word.to_owned()).collect()
}

/// What `launch` does: the program starts, the agent is told its pid, and
/// it is granted until it exits.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launched_program_is_granted_and_its_pid_returned() {
    let session = launching("launch-granted", &["sleep"]);
    let host = Host::new(&session.facts, &session.requests);
    let pid = host
        .launch(&words(&["sleep", "1"]), None)
        .expect("sleep is admitted");
    session.wait_for(|facts| granted(facts) == Some(&[pid][..]));
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop(());
}

/// A bare name admits only itself, never a path an agent chose, and the
/// refusal says what is admitted.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_the_list_does_not_name_is_refused_naming_the_list() {
    let session = launching("launch-not-admitted", &["sleep"]);
    let host = Host::new(&session.facts, &session.requests);
    let refused = host.launch(&words(&["/usr/bin/sleep", "30"]), None);
    assert_eq!(
        refused,
        Err(LaunchError::NotAdmitted {
            program: "/usr/bin/sleep".to_owned(),
            admitted: words(&["sleep"]),
        })
    );
    let said = refused.expect_err("refused").to_string();
    assert!(said.contains("may start only sleep"), "{said}");
    assert!(matches!(
        host.launch(&words(&["true"]), None),
        Err(LaunchError::NotAdmitted { .. })
    ));
    let facts = session.wait_for(|facts| granted(facts).is_some());
    assert_eq!(granted(&facts), Some(&[][..]), "nothing started");
    session.stop(());
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn launching_is_refused_when_off() {
    let session = Session::start(
        "launch-off",
        Backend::headless((800, 600))
            .granting()
            .launching(Rule::Off),
    );
    let host = Host::new(&session.facts, &session.requests);
    assert_eq!(
        host.launch(&words(&["sleep", "1"]), None),
        Err(LaunchError::Off)
    );
    session.stop(());
}

/// The person has walked away: nothing is started on their desk.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn launching_is_refused_while_locked() {
    let session = launching("launch-locked", &["sleep"]);
    let host = Host::new(&session.facts, &session.requests);
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    assert_eq!(
        host.launch(&words(&["sleep", "1"]), None),
        Err(LaunchError::Locked)
    );
    desk.unlock();
    session.stop((desk, queue));
}

/// Headless starts any program, as nobody sits at it to say otherwise. One
/// that will not start says why.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_that_will_not_start_says_why() {
    let session = Session::start("launch-not-started", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    host.launch(&words(&["sleep", "1"]), None)
        .expect("headless admits any program");
    match host.launch(&words(&["perspicax-no-such-program", "--flag"]), None) {
        Err(LaunchError::NotStarted(why)) => assert!(
            why.contains("perspicax-no-such-program --flag") && why.contains("PATH"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        host.launch(
            &words(&["sleep", "1"]),
            Some(Path::new("/nonexistent/perspicax"))
        ),
        Err(LaunchError::NotStarted(_))
    ));
    session.stop(());
}

/// A shell command that puts up [`window_probe`]'s window, as a program an
/// agent launched would start one: from a process of its own, under the
/// shell. The `; :` keeps the shell from becoming the probe.
fn probe_under_sh(session: &Session) -> Vec<String> {
    probe_under_sh_as(
        session,
        &std::env::current_exe().expect("this test's binary"),
    )
}

/// The same, put up by `program`, this binary under another name.
fn probe_under_sh_as(session: &Session, program: &Path) -> Vec<String> {
    words(&[
        "sh",
        "-c",
        &format!(
            "{PROBE}={} '{}' --exact window_probe --ignored --test-threads=1 >/dev/null; :",
            session.socket(),
            program.display()
        ),
    ])
}

/// The pid that drew the window titled `title`.
fn drawn_by(facts: &HostFacts, title: &str) -> Option<u32> {
    match &titled(facts, title)?.origin {
        perspicax_node::Origin::Process(process) => Some(process.pid),
        _ => None,
    }
}

fn kill(pid: u32) {
    let killed = Command::new("kill").arg(pid.to_string()).status();
    assert!(killed.is_ok_and(|status| status.success()), "kill {pid}");
}

/// What `flutter run` needs: the window is drawn by a process the launched
/// program started, and that is granted too.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_a_launched_program_starts_is_granted_while_it_runs() {
    let session = launching("lineage-granted", &["sh"]);
    let host = Host::new(&session.facts, &session.requests);
    let sh = host
        .launch(&probe_under_sh(&session), None)
        .expect("sh is admitted");
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    let probe = drawn_by(&facts, "probe").expect("drawn");
    assert_ne!(probe, sh, "drawn by the shell's child");
    session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.contains(&probe)));

    kill(probe);
    session.wait_for(|facts| granted(facts).is_some_and(|pids| !pids.contains(&probe)));
    session.stop(());
}

/// A launch's descendants are covered while it lives: once it is gone,
/// what it started is no longer the agent's, though it runs on.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_lineage_goes_when_the_launch_does() {
    let session = launching("lineage-ends", &["sh"]);
    let host = Host::new(&session.facts, &session.requests);
    let sh = host
        .launch(&probe_under_sh(&session), None)
        .expect("sh is admitted");
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    let probe = drawn_by(&facts, "probe").expect("drawn");
    session.wait_for(|facts| granted(facts).is_some_and(|pids| pids.len() == 2));

    kill(sh);
    let facts = session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    assert!(
        drawn_by(&facts, "probe").is_some(),
        "the window is still there, and no longer the agent's"
    );
    kill(probe);
    session.stop(());
}

/// Lineage is the launch's own: a window from elsewhere is not covered,
/// though a launch runs beside it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_the_launch_did_not_start_is_not_granted() {
    let session = launching("lineage-elsewhere", &["sleep"]);
    let host = Host::new(&session.facts, &session.requests);
    let sleep = host
        .launch(&words(&["sleep", "30"]), None)
        .expect("sleep is admitted");
    let mut probe = probe(&session);
    let facts = session.wait_for(|facts| granted(facts).is_some_and(|pids| !pids.is_empty()));
    assert_eq!(granted(&facts), Some(&[sleep][..]));
    probe.kill().expect("the probe is ours to kill");
    probe.wait().expect("and ours to reap");
    session.stop(());
}

/// What a binding marked `agent = true` starts is granted, and what that
/// starts is not, though a launch runs beside it: only the person's
/// `[agent] launch` bounds what a lineage may be.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_window_a_spawned_program_starts_is_not_granted() {
    let session = launching("lineage-spawned", &["sleep"]);
    let host = Host::new(&session.facts, &session.requests);
    host.launch(&words(&["sleep", "30"]), None)
        .expect("sleep is admitted");
    session.perform(Action::Spawn {
        command: probe_under_sh(&session),
        agent: true,
    });
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    let probe = drawn_by(&facts, "probe").expect("drawn");
    let granted_now = granted(&facts).expect("published").to_vec();
    assert_eq!(granted_now.len(), 2, "sleep and the shell: {granted_now:?}");
    assert!(!granted_now.contains(&probe));
    kill(probe);
    session.stop(());
}

/// What a seat's `[agent] never` names unless the person says otherwise.
fn never_ennius() -> Never {
    Never::new(vec![Program::parse("ennius")])
}

/// A desk that consents as a seat does, where an agent may start what
/// `launch` admits and is never given ennius.
fn withholding() -> Backend {
    Backend::headless((1280, 1024))
        .granting()
        .withholding(never_ennius())
}

/// The refusal a window of ennius's is given, whatever was granted.
fn never_granted() -> Result<(), Refusal> {
    Err(Refusal::NeverGranted {
        program: "ennius".to_owned(),
    })
}

/// [`window_probe`] run as `program`, put up as a binding or `--spawn`
/// starts it, told the socket by the session.
fn probe_command(program: &Path) -> Vec<String> {
    words(&[
        &program.display().to_string(),
        "--exact",
        "window_probe",
        "--ignored",
        "--test-threads=1",
    ])
}

/// The person's key on ennius's window grants nothing, and the window is
/// refused to the agent and not marked.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_key_does_not_grant_a_program_never_given() {
    let ennius = Renamed::called("ennius", "never-key");
    let session = Session::start("never-key", withholding());
    let mut probe = probe_as(&session, &ennius.0);
    session.perform(Action::ToggleAgent);
    // Heard in the order sent: once this is granted, the key has been too.
    session.perform(Action::Spawn {
        command: words(&["sleep", "30"]),
        agent: true,
    });
    let facts = session.wait_for(|facts| granted(facts).is_some_and(|pids| !pids.is_empty()));
    let pids = granted(&facts).expect("published");
    assert_eq!(pids.len(), 1, "sleep's alone: {pids:?}");
    assert!(!pids.contains(&probe.id()));
    let window = titled(&facts, "probe").expect("the probe's window");
    assert_eq!(facts.consent().check(&window.origin), never_granted());
    assert!(!outlined(&facts, "probe"));

    probe.kill().expect("the probe is ours to kill");
    probe.wait().expect("and ours to reap");
    session.stop(());
}

/// An agent asking to launch ennius is refused as never given, before any
/// rule is asked, and nothing starts: by its name, by its path, through a
/// link of another name to it, and through a link of its name to a file of
/// another, as an install that keeps versions side by side makes one. What
/// the rule admits still starts.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_launch_of_a_program_never_given_is_refused_whatever_the_rule_admits() {
    let ennius = Renamed::called("ennius", "never-launch");
    let link = ennius.link("e");
    let versioned = Renamed::called("ennius-0.3", "never-launch-versioned");
    let installed = versioned.link("ennius");
    let session = Session::start("never-launch", withholding().launching(Rule::Any));
    let host = Host::new(&session.facts, &session.requests);
    for program in [
        "ennius".to_owned(),
        ennius.0.display().to_string(),
        link.display().to_string(),
        installed.display().to_string(),
    ] {
        assert_eq!(
            host.launch(&words(&[&program, "gui"]), None),
            Err(LaunchError::NeverGranted {
                program: "ennius".to_owned()
            }),
            "{program}"
        );
    }
    let sleep = host
        .launch(&words(&["sleep", "30"]), None)
        .expect("any other program starts");
    session.wait_for(|facts| granted(facts) == Some(&[sleep][..]));
    session.stop(());
}

/// A launch covers what it starts, but not ennius: the shell is the
/// agent's, and the window ennius puts up under it is not.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_never_given_that_a_launch_starts_is_not_granted() {
    let ennius = Renamed::called("ennius", "never-lineage");
    let session = Session::start(
        "never-lineage",
        withholding().launching(Rule::Only(vec![Program::parse("sh")])),
    );
    let host = Host::new(&session.facts, &session.requests);
    let sh = host
        .launch(&probe_under_sh_as(&session, &ennius.0), None)
        .expect("sh is admitted");
    // Its grant is decided as it connects, before its window is drawn.
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    let probe = drawn_by(&facts, "probe").expect("drawn");
    assert_ne!(probe, sh, "drawn by the shell's child");
    assert_eq!(granted(&facts), Some(&[sh][..]));
    kill(probe);
    session.stop(());
}

/// A binding the person marked for the agent starts ennius through a link
/// of another name, which the config can't see through: it starts, since
/// the person asked, and is not granted.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_binding_for_the_agent_starts_a_program_never_given_and_grants_nothing() {
    let ennius = Renamed::called("ennius", "never-binding");
    let link = ennius.link("e");
    let session = Session::start_telling(
        "never-binding",
        withholding(),
        Vec::new(),
        vec![(PROBE.to_owned(), Session::socket_named("never-binding"))],
    );
    session.perform(Action::Spawn {
        command: probe_command(&link),
        agent: true,
    });
    // Started for the agent, it would be granted before its window maps.
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    assert_eq!(
        granted(&facts),
        Some(&[][..]),
        "started, and not the agent's"
    );
    let window = titled(&facts, "probe").expect("the probe's window");
    assert_eq!(facts.consent().check(&window.origin), never_granted());
    session.stop(());
}

/// `--spawn` starts ennius, and does not grant it.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_program_never_given_that_spawn_starts_is_not_granted() {
    let ennius = Renamed::called("ennius", "never-spawn");
    let session = Session::start_telling(
        "never-spawn",
        withholding(),
        vec![probe_command(&ennius.0)],
        vec![(PROBE.to_owned(), Session::socket_named("never-spawn"))],
    );
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    assert_eq!(
        granted(&facts),
        Some(&[][..]),
        "started, and not the agent's"
    );
    session.stop(());
}

/// A granted process that becomes ennius keeps its pid, and the grant on
/// it, but not the agent: the consent refuses its window by its program,
/// whatever pid is granted. The person's key still takes the grant back.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_granted_process_that_becomes_a_program_never_given_is_refused_and_taken_back() {
    let ennius = Renamed::called("ennius", "never-exec");
    let session = Session::start_telling(
        "never-exec",
        withholding(),
        vec![words(&[
            "sh",
            "-c",
            &format!(
                "exec '{}' --exact window_probe --ignored --test-threads=1 >/dev/null",
                ennius.0.display()
            ),
        ])],
        vec![(PROBE.to_owned(), Session::socket_named("never-exec"))],
    );
    let facts = session.wait_for(|facts| drawn_by(facts, "probe").is_some());
    let probe = drawn_by(&facts, "probe").expect("drawn");
    assert_eq!(
        granted(&facts),
        Some(&[probe][..]),
        "sh's grant, kept by exec"
    );
    let window = titled(&facts, "probe").expect("the probe's window");
    assert_eq!(facts.consent().check(&window.origin), never_granted());
    assert!(!outlined(&facts, "probe"));

    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| granted(facts).is_some_and(<[u32]>::is_empty));
    session.stop(());
}

/// Not a test: the window [`probe`] puts up, in a process of its own.
#[test]
#[ignore = "a window in a process of its own, for the tests that need one"]
fn window_probe() {
    let Ok(socket) = std::env::var(PROBE) else {
        return;
    };
    let (mut desk, mut queue, qh, _) = common::client_on(&socket);
    desk.open_window(&qh, "probe", "probe");
    loop {
        queue
            .blocking_dispatch(&mut desk)
            .expect("the compositor is there");
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_desk_that_does_not_consent_by_grant_consents_to_everyone_whatever_it_spawns() {
    let session = Session::start_spawning(
        "grant-everyone",
        Backend::headless((800, 600)),
        vec![vec!["sleep".to_owned(), "1".to_owned()]],
    );
    let facts = session.wait_for(|facts| facts.consent() != &Consent::Nobody);
    assert_eq!(facts.consent(), &Consent::Everyone);
    session.stop(());
}

/// Whether the window titled `title` is marked out, as its facts say: a
/// window that draws its own frame has strips round it only then.
fn outlined(facts: &HostFacts, title: &str) -> bool {
    titled(facts, title).is_some_and(|window| window.mapped && window.frame.len() == 4)
}

/// One window of this process's, `title`, on `session`, mapped.
fn one_window(
    session: &Session,
    title: &str,
) -> (
    common::Desk,
    wayland_client::EventQueue<common::Desk>,
    HostFacts,
) {
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, title, title);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| titled(facts, title).is_some_and(|window| window.mapped));
    (desk, queue, facts)
}

/// The line round a granted window is the compositor's own drawing, so it
/// covers what is under it on the compositor's word, as a titlebar does:
/// the edge of the window behind, which a person can no longer see.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_granted_windows_outline_covers_its_neighbour_on_the_hosts_word() {
    let session = granting("mark-covers");
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_window(&qh, "below", "below");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.open_window(&qh, "above", "above");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    let facts = session.wait_for(|facts| {
        ["below", "above"]
            .iter()
            .all(|title| titled(facts, title).is_some_and(|window| window.mapped))
    });
    let below = titled(&facts, "below").expect("mapped").clone();
    let above = titled(&facts, "above").expect("mapped").clone();

    // A node of the window behind, a pixel wide, two to the left of the
    // one in front: where the default line, three wide, is drawn.
    let (x, y) = (above.geometry.x0 - 2.0, above.geometry.y0 + 10.0);
    assert!(
        below.geometry.x0 < x - 1.0 && y + 10.0 < below.geometry.y1,
        "the cascade leaves the window behind there: {:?}, {:?}",
        below.geometry,
        above.geometry
    );
    let (left, top) = (below.geometry.x0, below.geometry.y0);
    let node = Rect::new(x - left, y - top, x + 1.0 - left, y + 10.0 - top);
    assert_eq!(
        judge(&facts, below.id, node).visibility,
        Visibility::Visible,
        "nothing is drawn there yet"
    );

    session.perform(Action::ToggleAgent);
    let facts = session.wait_for(|facts| outlined(facts, "above") && outlined(facts, "below"));
    let verdict = judge(&facts, below.id, node);
    assert_eq!(verdict.visibility, Visibility::Occluded { by: above.id });
    assert!(!verdict.unproven, "on the host's word");
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_revoked_window_loses_its_outline() {
    let session = granting("mark-revoked");
    let (desk, queue, facts) = one_window(&session, "mine");
    assert!(!outlined(&facts, "mine"), "not granted yet");

    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| outlined(facts, "mine"));
    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| titled(facts, "mine").is_some_and(|window| window.frame.is_empty()));
    session.stop((desk, queue));
}

/// A fullscreen window fills its monitor, and has no outside to draw a
/// line on but the next monitor.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_fullscreen_window_is_not_outlined() {
    // With a person at it: a desk nobody sits at fills no monitor.
    let session = Session::start(
        "mark-fullscreen",
        Backend::headless((1280, 1024)).with_person().granting(),
    );
    let (mut desk, mut queue, _) = one_window(&session, "mine");
    session.perform(Action::ToggleAgent);
    session.wait_for(|facts| outlined(facts, "mine"));

    desk.windows[0].set_fullscreen(None);
    until(&mut queue, &mut desk, |desk| {
        desk.offered == Some((1280, 1024))
    });
    // The client draws at the size it drew before, and is placed in the
    // corner of the monitor it was offered.
    let facts = session
        .wait_for(|facts| titled(facts, "mine").is_some_and(|window| window.frame.is_empty()));
    let mine = titled(&facts, "mine").expect("mapped");
    assert_eq!((mine.geometry.x0, mine.geometry.y0), (0.0, 0.0));
    assert_eq!(
        granted(&facts),
        Some(&[std::process::id()][..]),
        "still granted"
    );
    session.stop((desk, queue));
}

/// Where consent is everyone's, every window is one an agent may act on,
/// and a mark on every window would mark nothing out.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn nothing_is_outlined_where_consent_is_everyone() {
    let session = Session::start("mark-everyone", Backend::headless((1280, 1024)));
    let (desk, queue, facts) = one_window(&session, "mine");
    assert_eq!(facts.consent(), &Consent::Everyone);
    assert!(titled(&facts, "mine").expect("mapped").frame.is_empty());
    session.stop((desk, queue));
}
