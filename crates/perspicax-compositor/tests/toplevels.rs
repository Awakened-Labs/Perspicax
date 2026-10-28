//! The windows, as a taskbar's list is told them, by a compositor with no
//! screen.
//!
//! A window opening, being retitled and closing reaches a client holding
//! `ext-foreign-toplevel-list-v1`. `[protocols]` decides who gets the list
//! at all: nobody, any client, or the programs it names. A rule narrowed
//! while a client holds the list takes it back. And while the session is
//! locked the list hears nothing, then everything at once when it is lifted.
//!
//! The client is this test binary, in this process, so an allowlist naming
//! `current_exe()` admits it and one naming anything else does not. Like the
//! other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.
//!
//! A path in a rule admits only the file at that path: a copy of this binary
//! mounted at another program's path, in mounts of its own, reads as that
//! path and is still refused (#178).

mod common;

use std::process::{Command as Process, Stdio};

use common::{Session, advertised, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_policy::{Access, Program, Protocol, Rule, Shape};

const LIST: &str = "ext_foreign_toplevel_list_v1";

/// Set, to the compositor's socket, for the copy of this binary that
/// [`list_probe`] runs in.
const PROBE: &str = "PERSPICAX_LIST_PROBE";

fn backend(access: Access) -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (1280, 1024))],
        workspaces: Shape::default(),
        access,
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

fn this_binary() -> String {
    std::env::current_exe()
        .expect("the test binary")
        .to_string_lossy()
        .into_owned()
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_list_announces_a_window_its_title_change_and_its_close() {
    let session = Session::start("list", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();

    desk.open_window(&qh, "first", "org.example.First");
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    desk.bind_list(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first"]
    });
    let first = desk.listed_by_title("first").unwrap().clone();
    assert_eq!(first.app_id, "org.example.First");
    assert_eq!(first.identifier.len(), 32);

    // A window opened while the list is held arrives on its own.
    desk.open_window(&qh, "second", "org.example.Second");
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first", "second"]
    });

    // Retitled without drawing again: titles are not double-buffered.
    desk.windows[0].set_title("renamed");
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["renamed", "second"]
    });
    assert_eq!(
        desk.listed_by_title("renamed").unwrap().identifier,
        first.identifier,
        "the same window, under its new name"
    );
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("renamed"))
    });

    desk.windows.remove(1);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["renamed"]
    });

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_list_is_not_advertised_when_off() {
    let access = Access::open().with(Protocol::ForeignToplevelList, Rule::Off);
    let session = Session::start("list-off", backend(access));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, LIST));
    assert!(
        advertised(&globals, "wl_compositor"),
        "the registry was read at all"
    );
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn an_allowlist_admits_the_programs_it_names_and_no_other() {
    let naming = |entry: String| {
        Access::open().with(
            Protocol::ForeignToplevelList,
            Rule::Only(vec![Program::parse(&entry)]),
        )
    };

    let session = Session::start("list-us", backend(naming(this_binary())));
    let (desk, queue, _qh, globals) = session.client();
    assert!(advertised(&globals, LIST), "named by full path");
    session.stop((desk, queue));

    let session = Session::start("list-them", backend(naming("waybar".to_owned())));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, LIST), "not waybar");
    session.stop((desk, queue));
}

/// Whether this machine lets a program make mounts of its own without root:
/// `unshare -rm`, as a rootless container does. Many containers do not.
fn may_unshare() -> bool {
    Process::new("unshare")
        .args(["-rm", "true"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether [`list_probe`], run as `program` with `args` before it, was
/// offered the list by `session`.
fn probe_offered(session: &Session, program: &str, args: &[&str]) -> bool {
    let status = Process::new(program)
        .args(args)
        .args(["--exact", "list_probe", "--ignored", "--test-threads=1"])
        .env(PROBE, session.socket())
        .stdout(Stdio::null())
        .status()
        .expect("the probe runs");
    match status.code() {
        Some(0) => true,
        Some(1) => false,
        other => panic!("the probe failed: {other:?}"),
    }
}

/// A program with mounts of its own can put any file at any path, and the
/// kernel then names what it runs by that path. Here a copy of this binary
/// is mounted where `env` is and run from there: the compositor reads its
/// executable as `env`'s path, and a rule naming that path still does not
/// admit it, since the file at that path is not the one it runs.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_path_admits_no_other_file_mounted_there() {
    if !may_unshare() {
        eprintln!("skipped: this machine lets no program make mounts of its own without root");
        return;
    }
    let env = std::fs::canonicalize("/usr/bin/env").expect("env is in coreutils");
    let env = env.to_string_lossy().into_owned();
    let me = this_binary();
    // `$1` mounted at `$2`, then run from there with what follows.
    let script = r#"from=$1; to=$2; shift 2; mount --bind "$from" "$to" && exec "$to" "$@""#;
    let mounted = ["-rm", "sh", "-c", script, "sh", &me, &env];
    let naming = |program: Program| {
        Access::open().with(Protocol::ForeignToplevelList, Rule::Only(vec![program]))
    };

    let by_name = Program::parse(env.rsplit('/').next().expect("a file name"));
    let session = Session::start("list-env-name", backend(naming(by_name)));
    assert!(
        probe_offered(&session, "unshare", &mounted),
        "read as env, a name admits it: a name was never more"
    );
    session.stop(());

    let session = Session::start("list-env-path", backend(naming(Program::parse(&env))));
    assert!(
        !probe_offered(&session, "unshare", &mounted),
        "env's path, but not env's file"
    );
    session.stop(());

    let session = Session::start("list-own-path", backend(naming(Program::parse(&me))));
    assert!(
        probe_offered(&session, &me, &[]),
        "its own path, and its own file"
    );
    session.stop(());
}

/// Not a test: a client in a process of its own, for
/// [`a_path_admits_no_other_file_mounted_there`]. Exits 0 if the compositor
/// offered it the list, and 1 if not.
#[test]
#[ignore = "a client in a process of its own, for the tests that need one"]
fn list_probe() {
    let Ok(socket) = std::env::var(PROBE) else {
        return;
    };
    let (_desk, _queue, _qh, globals) = common::client_on(&socket);
    std::process::exit(if advertised(&globals, LIST) { 0 } else { 1 });
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_narrowed_rule_takes_the_list_back() {
    let session = Session::start("list-narrowed", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    desk.bind_list(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first"]
    });

    session.command(Command::Protocols(Access::open().with(
        Protocol::ForeignToplevelList,
        Rule::Only(vec![Program::parse("waybar")]),
    )));
    until(&mut queue, &mut desk, |desk| desk.list_finished);
    assert!(desk.listed_titles().is_empty(), "every window closed first");

    // And nothing new reaches it.
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    assert!(desk.listed_titles().is_empty());

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_list_hears_nothing_while_locked_and_everything_after() {
    let session = Session::start("list-locked", backend(Access::open()));
    let (mut desk, mut queue, qh, globals) = session.client();
    desk.open_window(&qh, "first", "first");
    desk.bind_list(&globals, &qh);
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["first"]
    });

    desk.lock(&globals, &qh);
    until(&mut queue, &mut desk, |desk| desk.locked);
    desk.windows[0].set_title("behind the lock");
    desk.open_window(&qh, "second", "second");
    until(&mut queue, &mut desk, |desk| desk.drawn == 2);
    session.wait_for(|facts| {
        facts
            .surfaces()
            .iter()
            .any(|surface| surface.title.as_deref() == Some("second"))
    });
    queue.roundtrip(&mut desk).expect("dispatch");
    assert_eq!(desk.listed_titles(), ["first"], "nothing while locked");

    desk.unlock();
    until(&mut queue, &mut desk, |desk| {
        desk.listed_titles() == ["behind the lock", "second"]
    });

    session.stop((desk, queue));
}
