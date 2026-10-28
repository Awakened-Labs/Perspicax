//! Who may lock the session, by a compositor with no screen:
//! `[protocols] session-lock` (#178).
//!
//! A lock program is typed the password, so a client that could lock could
//! put up a lock screen of its own and be typed it instead. Only a program
//! the rule admits is shown `ext_session_lock_manager_v1`, and one that bound
//! it before a reload narrowed the rule is refused its lock, told `finished`,
//! whether it asks to lock or to take over a session whose lock has gone. A
//! reload never takes the session from the lock holding it, which still
//! unlocks: nobody else could.
//!
//! A program called what a lock program the rule names is called, refused,
//! is told to a shell bound at version 8, to say on the screen: a lock screen
//! came up that was not the one perspicax lets lock it.
//!
//! The clients are this test binary, so a rule naming `current_exe()` admits
//! them. By a bare name read as the system's copy it does not: the binary
//! is somewhere the person running the tests could have written. Like the
//! other live tests these bind a real Wayland socket, so they need
//! `XDG_RUNTIME_DIR`, and are `#[ignore]`d for `ci/live-tests.sh` to run.

mod common;

use std::{
    net::Shutdown,
    process::{Command as Process, Stdio},
    thread,
    time::{Duration, Instant},
};

use common::{Desk, Ear, Renamed, Session, Told, advertised, client_over, heard, until};
use perspicax_compositor::{Backend, Command, Virtual};
use perspicax_index::{HostFacts, SurfaceKind};
use perspicax_node::SurfaceId;
use perspicax_policy::{Access, Program, Protocol, Rule, Shape};
use perspicax_protocols::shell::v1::client::perspicax_shell_v1::Refusal;
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{Connection, EventQueue, Proxy as _, QueueHandle, globals::GlobalList};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;

const LOCK: &str = "ext_session_lock_manager_v1";
/// Set, to the compositor's socket, for the copy of this binary that
/// [`lock_probe`] runs in.
const PROBE: &str = "PERSPICAX_LOCK_PROBE";
const PAGE: u32 = 0xff33_6699;
const COVER: u32 = 0xff20_2020;

fn backend(rule: Rule) -> Backend {
    Backend::Headless {
        outputs: vec![Virtual::numbered(1, (1280, 800))],
        workspaces: Shape::default(),
        access: Access::open().with(Protocol::SessionLock, rule),
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

/// This test binary's file name: `lock_rule-0123abcd`.
fn its_name() -> String {
    this_binary()
        .rsplit('/')
        .next()
        .expect("a file name")
        .to_owned()
}

fn only(program: Program) -> Rule {
    Rule::Only(vec![program])
}

/// Put `rule` in force for the lock, as a reload does, and wait until a
/// client connecting now is `shown` the lock's global, or is not.
fn reload(session: &Session, rule: Rule, shown: bool) {
    session.command(Command::Protocols(
        Access::open().with(Protocol::SessionLock, rule),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (desk, queue, _qh, globals) = session.client();
        let now = advertised(&globals, LOCK);
        drop((desk, queue));
        if now == shown {
            return;
        }
        assert!(Instant::now() < deadline, "the reload never took");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A client with a window called "page", drawn, and a keyboard, which the
/// page has: it has it again only once the session is unlocked. Returns
/// what the keyboard hears.
fn page(session: &Session) -> (Desk, EventQueue<Desk>, Ear) {
    let (mut desk, mut queue, qh, globals) = session.client();
    let ear = desk.bind_keyboard(&globals, &qh);
    desk.open_coloured(&qh, "page", "page", PAGE);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let window = desk.windows[0].wl_surface().id();
    until(&mut queue, &mut desk, |_| {
        heard(&ear).entered.as_ref() == Some(&window)
    });
    (desk, queue, ear)
}

/// Whether the page has the keyboard, as far as everything the compositor
/// has sent says.
fn page_has_keyboard(desk: &mut Desk, queue: &mut EventQueue<Desk>, ear: &Ear) -> bool {
    queue.roundtrip(desk).expect("dispatch");
    heard(ear).entered.is_some()
}

/// Ask `locker` to lock the session, as swaylock does, and wait for the
/// answer: `locked`, or `finished`.
fn ask_to_lock(
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    globals: &GlobalList,
) {
    until(queue, locker, |locker| !locker.outputs_empty());
    locker.lock(globals, qh);
    until(queue, locker, |locker| {
        locker.locked || locker.lock_finished
    });
}

/// Bind the lock's global now, to ask with later, as a program that started
/// before a reload did.
fn bound(
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
    globals: &GlobalList,
) -> ExtSessionLockManagerV1 {
    let manager = globals
        .bind::<ExtSessionLockManagerV1, _, _>(qh, 1..=1, ())
        .expect(LOCK);
    queue.roundtrip(locker).expect("bound");
    manager
}

/// Ask through a global bound before, and wait for the answer.
fn ask_through(
    manager: &ExtSessionLockManagerV1,
    locker: &mut Desk,
    queue: &mut EventQueue<Desk>,
    qh: &QueueHandle<Desk>,
) {
    locker.lock = Some(manager.lock(qh, ()));
    until(queue, locker, |locker| {
        locker.locked || locker.lock_finished
    });
}

/// Cover HEADLESS-1 with a lock surface of `locker`'s lock, and wait until
/// it is drawn.
fn cover(locker: &mut Desk, queue: &mut EventQueue<Desk>, qh: &QueueHandle<Desk>) {
    let drawn = locker.covers_drawn;
    locker.cover(qh, "HEADLESS-1", COVER);
    until(queue, locker, |locker| locker.covers_drawn > drawn);
}

/// The lock surfaces the facts say cover a monitor.
fn covers(facts: &HostFacts) -> Vec<SurfaceId> {
    facts
        .surfaces()
        .iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::LockCover))
        .map(|surface| surface.id)
        .collect()
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn no_lock_is_offered_when_the_rule_is_off() {
    let session = Session::start("lock-rule-off", backend(Rule::Off));
    let (desk, queue, _qh, globals) = session.client();
    assert!(!advertised(&globals, LOCK));
    assert!(
        advertised(&globals, "wl_compositor"),
        "the registry was read at all"
    );
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_program_named_by_its_path_locks() {
    let session = Session::start(
        "lock-rule-path",
        backend(only(Program::parse(&this_binary()))),
    );
    let (mut locker, mut queue, qh, globals) = session.client();
    assert!(advertised(&globals, LOCK), "named by its path");
    ask_to_lock(&mut locker, &mut queue, &qh, &globals);
    assert!(locker.locked);
    locker.unlock();
    queue.roundtrip(&mut locker).expect("unlocked");
    session.stop((locker, queue));
}

/// A name in `session-lock` is the system's copy of that program, and this
/// binary is not one: by the same name, read as any program's, it would be.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_name_admits_only_the_copy_the_system_installed() {
    let session = Session::start(
        "lock-rule-installed",
        backend(only(Program::parse(&its_name()).installed())),
    );
    let (desk, queue, _qh, globals) = session.client();
    assert!(
        !advertised(&globals, LOCK),
        "this binary is the person's to write, not the system's"
    );
    session.stop((desk, queue));

    let session = Session::start(
        "lock-rule-any-copy",
        backend(only(Program::parse(&its_name()))),
    );
    let (desk, queue, _qh, globals) = session.client();
    assert!(
        advertised(&globals, LOCK),
        "the same name, as any program's"
    );
    session.stop((desk, queue));
}

/// The rule narrowed while a lock holds the session: nobody new may lock,
/// and the lock holding it keeps it, its cover still covering, and unlocks.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_reload_never_takes_the_session_from_the_lock_holding_it() {
    let session = Session::start("lock-rule-reload-held", backend(Rule::Any));
    let (mut desk, mut queue, ear) = page(&session);
    let (mut holder, mut holder_queue, holder_qh, holder_globals) = session.client();
    ask_to_lock(&mut holder, &mut holder_queue, &holder_qh, &holder_globals);
    assert!(holder.locked, "the lock holds the session");
    cover(&mut holder, &mut holder_queue, &holder_qh);
    session.wait_for(|facts| covers(facts).len() == 1);

    reload(&session, Rule::Off, false);
    assert_eq!(
        covers(&session.wait_for(|_| true)).len(),
        1,
        "its cover still covers"
    );
    assert!(
        !page_has_keyboard(&mut desk, &mut queue, &ear),
        "still locked"
    );

    holder.unlock();
    holder_queue.roundtrip(&mut holder).expect("unlocked");
    session.wait_for(|facts| covers(facts).is_empty());
    assert!(
        page_has_keyboard(&mut desk, &mut queue, &ear),
        "unlocked, by the lock holding it"
    );

    session.stop(((desk, queue), (holder, holder_queue)));
}

/// A program that bound the global while any program might lock, and asks
/// once a reload has narrowed the rule, is refused as a second lock is.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_asked_for_after_the_rule_narrowed_is_refused() {
    let session = Session::start("lock-rule-bound-before", backend(Rule::Any));
    let (mut desk, mut queue, ear) = page(&session);
    let (mut locker, mut locker_queue, locker_qh, locker_globals) = session.client();
    until(&mut locker_queue, &mut locker, |locker| {
        !locker.outputs_empty()
    });
    let manager = bound(&mut locker, &mut locker_queue, &locker_qh, &locker_globals);

    reload(&session, Rule::Off, false);
    ask_through(&manager, &mut locker, &mut locker_queue, &locker_qh);
    assert!(locker.lock_finished && !locker.locked, "refused");
    assert!(
        page_has_keyboard(&mut desk, &mut queue, &ear),
        "never locked"
    );

    session.stop(((desk, queue), (locker, locker_queue)));
}

/// A lock gone without unlocking leaves the session locked for the next
/// lock to take over (#126): never one the rule no longer admits, whatever
/// it bound before. Once the rule admits a program again, it takes the
/// session over and unlocks.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_gone_is_taken_over_only_by_what_the_rule_admits() {
    let session = Session::start("lock-rule-taken-over", backend(Rule::Any));
    let (mut desk, mut queue, ear) = page(&session);

    // The holder, on a socket the test can cut, as a crash cuts swaylock's.
    let socket = common::stream(session.socket());
    let cut = socket.try_clone().expect("a second handle on the socket");
    let connection = Connection::from_socket(socket).expect("a Wayland connection");
    let (mut holder, mut holder_queue, holder_qh, holder_globals) = client_over(&connection);
    ask_to_lock(&mut holder, &mut holder_queue, &holder_qh, &holder_globals);
    assert!(holder.locked, "the lock holds the session");
    cover(&mut holder, &mut holder_queue, &holder_qh);
    session.wait_for(|facts| covers(facts).len() == 1);

    // Another program binds the global while any program might lock.
    let (mut taker, mut taker_queue, taker_qh, taker_globals) = session.client();
    until(&mut taker_queue, &mut taker, |taker| !taker.outputs_empty());
    let manager = bound(&mut taker, &mut taker_queue, &taker_qh, &taker_globals);

    // The rule narrows, and the holder dies: the session stays locked.
    reload(
        &session,
        only(Program::parse(&its_name()).installed()),
        false,
    );
    cut.shutdown(Shutdown::Both).expect("the connection cut");
    session.wait_for(|facts| covers(facts).is_empty());
    assert!(
        !page_has_keyboard(&mut desk, &mut queue, &ear),
        "still locked"
    );

    ask_through(&manager, &mut taker, &mut taker_queue, &taker_qh);
    assert!(
        taker.lock_finished && !taker.locked,
        "a program the rule no longer admits is refused the session"
    );
    assert!(
        !page_has_keyboard(&mut desk, &mut queue, &ear),
        "still locked"
    );

    reload(&session, Rule::Any, true);
    let (mut rescuer, mut rescuer_queue, rescuer_qh, rescuer_globals) = session.client();
    ask_to_lock(
        &mut rescuer,
        &mut rescuer_queue,
        &rescuer_qh,
        &rescuer_globals,
    );
    assert!(rescuer.locked, "admitted, the session taken over");
    rescuer.unlock();
    rescuer_queue.roundtrip(&mut rescuer).expect("unlocked");
    assert!(
        page_has_keyboard(&mut desk, &mut queue, &ear),
        "and unlocked"
    );

    session.stop((
        (desk, queue),
        (taker, taker_queue),
        (rescuer, rescuer_queue),
    ));
}

/// Long enough for anything the compositor was going to say to arrive.
fn settle(queue: &mut EventQueue<Desk>, desk: &mut Desk) {
    queue.roundtrip(desk).expect("dispatch");
    thread::sleep(Duration::from_millis(200));
    queue.roundtrip(desk).expect("dispatch");
}

/// A copy of this binary called swaylock, where the person running the
/// tests could have written it: a lock program not the system's, or a
/// program passing itself off as one. It is refused the global, and a shell
/// is told, by the rule's word for it; one from before version 8 is not.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_lock_program_refused_is_told_to_a_shell_that_knows_of_it() {
    let swaylock = Renamed::called("swaylock", "lock-refused");
    let session = Session::start(
        "lock-rule-told",
        backend(only(Program::parse("swaylock").installed())),
    );
    let (mut shell, mut shell_queue, shell_qh, shell_globals) = session.client();
    shell.bind_shell(&shell_globals, &shell_qh);
    shell_queue.roundtrip(&mut shell).expect("bound");
    let (mut old, mut old_queue, old_qh, old_globals) = session.client();
    old.bind_shell_v7(&old_globals, &old_qh);
    old_queue.roundtrip(&mut old).expect("bound");

    let status = Process::new(&swaylock.0)
        .args(["--exact", "lock_probe", "--ignored", "--test-threads=1"])
        .env(PROBE, session.socket())
        .stdout(Stdio::null())
        .status()
        .expect("the probe runs");
    assert_eq!(status.code(), Some(1), "not shown the lock's global");
    until(&mut shell_queue, &mut shell, |shell| !shell.told.is_empty());
    assert_eq!(
        shell.told,
        [Told::Refused(Ok(Refusal::Lock), "swaylock".to_owned())],
        "by the rule's word for it, never the path it ran from"
    );
    settle(&mut old_queue, &mut old);
    assert!(old.told.is_empty(), "{:?}", old.told);

    // A program not called what the rule names is nobody's business.
    let other = Renamed::called("firefox", "lock-unnamed");
    let status = Process::new(&other.0)
        .args(["--exact", "lock_probe", "--ignored", "--test-threads=1"])
        .env(PROBE, session.socket())
        .stdout(Stdio::null())
        .status()
        .expect("the probe runs");
    assert_eq!(status.code(), Some(1));
    settle(&mut shell_queue, &mut shell);
    assert_eq!(shell.told.len(), 1, "{:?}", shell.told);

    session.stop(((shell, shell_queue), (old, old_queue)));
    drop((swaylock, other));
}

/// Not a test: a client in a process of its own, for
/// [`a_lock_program_refused_is_told_to_a_shell_that_knows_of_it`]. Exits 0
/// if the compositor showed it the lock's global, and 1 if not.
#[test]
#[ignore = "a client in a process of its own, for the tests that need one"]
fn lock_probe() {
    let Ok(socket) = std::env::var(PROBE) else {
        return;
    };
    let (_desk, _queue, _qh, globals) = common::client_on(&socket);
    std::process::exit(if advertised(&globals, LOCK) { 0 } else { 1 });
}
