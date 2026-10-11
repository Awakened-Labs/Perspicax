//! `perspicax --mcp-socket`, the binary itself: what it refuses before it
//! starts anything, `perspicax attach` carrying a conversation, and a headless
//! session serving one conversation after another while it runs on (issue #30).
//! And `--mcp`'s wire, which nothing but the agent interface reaches (#43),
//! and which perspicax refuses to serve on when the session bus holds it too
//! (#52).
//!
//! The refusals need nothing but a filesystem, because the socket is bound
//! before the accessibility registry is touched, and `attach` is tested against
//! a socket this test serves over a desk with no compositor behind it; all of
//! them run wherever the tests do. The sessions need a live accessibility bus, so they are `#[ignore]`d,
//! and `ci/live-tests.sh` runs them with `--include-ignored`.

use std::{
    fs,
    io::{BufRead as _, BufReader, Read, Write as _},
    os::{
        fd::AsRawFd as _,
        unix::{fs::PermissionsExt as _, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::Arc,
    sync::Mutex,
    sync::atomic::{AtomicUsize, Ordering},
    sync::mpsc,
    time::{Duration, Instant},
};

use perspicax::desk::Desk;
use perspicax_compositor::{Facts, Host, Requests};
use serde_json::{Value, json};

/// How long anything here gets before the test gives up on it.
const PATIENCE: Duration = Duration::from_secs(10);

/// The binary under test.
fn perspicax() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_perspicax"));
    command.stdin(Stdio::null());
    command
}

/// A directory of this test's own, emptied and removed when dropped. Short,
/// because a socket's path has to fit in 108 bytes.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pxa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).expect("a scratch directory");
        Self(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        let _ = fs::remove_dir(&self.0);
    }
}

/// Run `command` to its end, which must come within [`PATIENCE`].
fn finished(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("perspicax starts");
    exited(&mut child, PATIENCE);
    child.wait_with_output().expect("its output")
}

/// Wait for `child` to exit, killing it if it has not within `patience`.
fn exited(child: &mut Child, patience: Duration) -> ExitStatus {
    let deadline = Instant::now() + patience;
    loop {
        if let Some(status) = child.try_wait().expect("a child to wait on") {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("perspicax was still running after {patience:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_socket_another_session_serves_is_refused_before_anything_starts() {
    let scratch = Scratch::new();
    let path = scratch.join("mcp");
    // Listening, and that is enough: a connection is answered by the kernel
    // whether or not anybody gets round to accepting it.
    let (_served, _listener) = perspicax_mcp::Socket::bind(&path).expect("a socket");

    let output = finished(perspicax().args([
        "--headless",
        "--mcp-socket",
        path.to_str().expect("UTF-8"),
        "--run-for",
        "5",
    ]));
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains(&format!("pid {}", std::process::id())),
        "the refusal names who serves the socket: {said}"
    );
    assert!(path.exists(), "the other session's socket is left alone");
}

#[test]
fn a_file_that_is_not_a_socket_is_left_alone() {
    let scratch = Scratch::new();
    let path = scratch.join("notes");
    fs::write(&path, "somebody's").expect("a file");

    let output = finished(perspicax().args([
        "--headless",
        "--mcp-socket",
        path.to_str().expect("UTF-8"),
        "--run-for",
        "5",
    ]));
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("not a socket"), "{said}");
    assert_eq!(
        fs::read_to_string(&path).expect("still there"),
        "somebody's"
    );
}

/// Under `2>&1` stderr is the agent's wire too, and every program perspicax
/// starts prints to stderr, so any of them could print a reply perspicax did
/// not send (issue #52). Refused before anything starts, on the wire itself,
/// where the client will read it.
#[test]
fn a_stderr_that_is_the_wire_too_is_refused_before_anything_starts() {
    let (wire, end) = std::io::pipe().expect("a pipe for the wire");
    let mut session = perspicax()
        .args(["--headless", "--mcp", "--run-for", "5"])
        .stdout(end.try_clone().expect("a second handle"))
        .stderr(end)
        .spawn()
        .expect("perspicax starts");
    let said = everything(wire);
    let status = exited(&mut session, PATIENCE);
    let said = said.recv_timeout(PATIENCE).expect("the wire closes");
    // Not the exit alone: with no accessibility registry to start, a run that
    // was not refused fails as well.
    assert!(!status.success(), "{said}");
    for named in ["stderr is the agent's wire too", "--mcp-socket"] {
        assert!(said.contains(named), "the refusal says {named:?}: {said}");
    }
}

/// Serve a socket at `path` over a desk with nothing on it and no compositor
/// behind it, on a thread of its own, for as long as the test runs.
fn serve_an_empty_desk(path: &Path) -> perspicax_mcp::Socket {
    let (socket, listener) = perspicax_mcp::Socket::bind(path).expect("a socket");
    std::thread::spawn(move || {
        let facts = Facts::new();
        let desk = Arc::new(Desk::new(&facts, &Host::new(&facts, &Requests::new())));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let Err(error) = runtime.block_on(perspicax_mcp::serve_socket(desk, listener));
        panic!("the socket stopped being served: {error}");
    });
    socket
}

#[test]
fn attach_carries_a_conversation_to_the_sessions_socket_and_ends_with_it() {
    let scratch = Scratch::new();
    let path = scratch.join("mcp");
    let _socket = serve_an_empty_desk(&path);

    let mut attached = perspicax()
        .arg("attach")
        .env("PERSPICAX_MCP_SOCKET", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("perspicax attach starts");
    let mut asking = attached.stdin.take().expect("its stdin");
    let mut answers = BufReader::new(attached.stdout.take().expect("its stdout"));
    let mut answer = || {
        let mut line = String::new();
        answers.read_line(&mut line).expect("an answer");
        serde_json::from_str::<Value>(&line).expect("JSON")
    };

    for message in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "perspicax attach test", "version": "0" },
        }}),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "window_list" } }),
    ] {
        writeln!(asking, "{message}").expect("attach is listening");
    }
    assert_eq!(answer()["result"]["serverInfo"]["name"], "perspicax");
    assert_eq!(answer()["result"]["structuredContent"]["count"], 0);

    // The client is done: its conversation ends, and so does the wire.
    drop(asking);
    let status = exited(&mut attached, PATIENCE);
    assert!(status.success(), "{status}");
}

#[test]
fn attach_in_a_session_serving_no_socket_says_so() {
    let output = finished(perspicax().arg("attach").env("PERSPICAX_MCP_SOCKET", ""));
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("serves no agent interface"), "{said}");
}

/// One connection to the socket, spoken to a line at a time.
struct Wire {
    lines: BufReader<UnixStream>,
    out: UnixStream,
}

impl Wire {
    fn to(path: &Path) -> Self {
        let stream = UnixStream::connect(path).expect("the socket answers");
        stream
            .set_read_timeout(Some(PATIENCE))
            .expect("a read timeout");
        Self {
            out: stream.try_clone().expect("a second handle"),
            lines: BufReader::new(stream),
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.out, "{message}").expect("the server is listening");
    }

    /// The next frame, or `None` once the server has closed the connection.
    fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = self.lines.read_line(&mut line).expect("a frame in time");
        (read > 0).then(|| serde_json::from_str(&line).expect("the server writes JSON"))
    }

    /// `initialize`, and the server's answer to it, whatever it is.
    fn initialize(&mut self) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "perspicax agent socket test", "version": "0" },
            },
        }));
        self.recv().expect("an answer to initialize")
    }

    /// A conversation, once whichever one came before it has let go.
    fn converse(path: &Path) -> Self {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let mut wire = Self::to(path);
            let answer = wire.initialize();
            if answer.get("error").is_none() {
                wire.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
                return wire;
            }
            assert!(Instant::now() < deadline, "never let in: {answer}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn window_count(&mut self, id: u64) -> u64 {
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "window_list" },
        }));
        let answer = self.recv().expect("an answer to window_list");
        answer["result"]["structuredContent"]["count"]
            .as_u64()
            .unwrap_or_else(|| panic!("window_list counts: {answer}"))
    }
}

/// Wait for `ready`, or fail saying what never happened.
fn eventually(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !ready() {
        assert!(Instant::now() < deadline, "{what}, after {PATIENCE:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "needs a live accessibility bus"]
fn a_headless_session_serves_conversation_after_conversation_and_tidies_up() {
    let scratch = Scratch::new();
    let path = scratch.join("mcp");
    let report = scratch.join("report");
    fs::write(
        &report,
        "#!/bin/sh\nprintf '%s' \"$PERSPICAX_MCP_SOCKET\" > \"$(dirname \"$0\")/seen\"\n",
    )
    .expect("a script");
    fs::set_permissions(&report, fs::Permissions::from_mode(0o755)).expect("executable");

    let mut session = perspicax()
        .args([
            "--headless",
            "--mcp-socket",
            path.to_str().expect("UTF-8"),
            "--spawn",
            report.to_str().expect("UTF-8"),
            "--run-for",
            "20",
        ])
        .stdout(Stdio::null())
        .spawn()
        .expect("perspicax starts");
    let up = |session: &mut Child| {
        assert!(
            session.try_wait().expect("a child").is_none(),
            "the session ended"
        );
    };

    eventually("no socket", || path.exists());
    let mode = fs::metadata(&path)
        .expect("the socket")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    // What a program the session starts is told.
    let seen = scratch.join("seen");
    eventually("the spawned program was never told", || seen.exists());
    assert_eq!(
        fs::read_to_string(&seen).expect("what it was told"),
        path.to_str().expect("UTF-8")
    );

    let mut first = Wire::converse(&path);
    assert_eq!(first.window_count(2), 0);

    // A second agent, while the first is talking.
    let mut second = Wire::to(&path);
    let refused = second.initialize();
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|reason| reason.contains("one at a time")),
        "{refused}"
    );
    assert!(second.recv().is_none(), "a refused connection is closed");
    assert_eq!(first.window_count(3), 0, "the first goes on");
    up(&mut session);

    // The first leaves, and another agent comes.
    drop(first);
    let mut third = Wire::converse(&path);
    assert_eq!(third.window_count(2), 0);
    drop(third);
    up(&mut session);

    let status = exited(&mut session, Duration::from_secs(40));
    assert!(status.success(), "{status}");
    assert!(!path.exists(), "the socket outlived its session");
}

/// Where `fd` of process `pid` points, as `/proc` says: `/dev/null`, or
/// `pipe:[N]`. `None` for a process or descriptor that is not there (yet).
fn link(pid: &str, fd: impl std::fmt::Display) -> Option<String> {
    fs::read_link(format!("/proc/{pid}/fd/{fd}"))
        .ok()?
        .into_os_string()
        .into_string()
        .ok()
}

/// Under `--mcp`, nothing reaches the agent's wire but the agent interface
/// (issue #43): not the programs the session starts, which read nothing and
/// print to stderr, and not anything in perspicax itself, whose fds 0 and 1
/// are `/dev/null` and stderr once it has taken the wire for its own.
#[test]
#[ignore = "needs a live accessibility bus"]
fn the_agents_wire_carries_nothing_but_the_agent_interface() {
    // Each says where its own fd 0 or 1 points, on what it was given for a
    // stdout: whatever `--spawn` started could print to the wire before.
    let mut session = perspicax()
        .args(["--headless", "--mcp"])
        .args(["--spawn", "readlink /proc/self/fd/0"])
        .args(["--spawn", "readlink /proc/self/fd/1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("perspicax starts");
    let pid = session.id().to_string();
    let log = link(
        "self",
        session.stderr.as_ref().expect("its stderr").as_raw_fd(),
    )
    .expect("a pipe for its stderr");

    // Read on threads, so a wire that never answers or never closes fails the
    // test rather than hanging it.
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr = session.stderr.take().expect("its stderr");
    std::thread::spawn({
        let said = Arc::clone(&said);
        move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                said.lock().expect("the lines said").push(line);
            }
        }
    });
    let (tell, answers) = mpsc::channel::<String>();
    let stdout = session.stdout.take().expect("its stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tell.send(line).is_err() {
                break;
            }
        }
    });

    // perspicax's own: a `println!` reaches stderr, and a read reads nothing.
    eventually("perspicax kept fds 0 and 1 for the wire", || {
        link(&pid, 0).as_deref() == Some("/dev/null") && link(&pid, 1).as_ref() == Some(&log)
    });
    // What it started. Before stdin closes: the server answers before the
    // programs are started, and the agent leaving ends the run.
    eventually("the spawned programs never said where they print", || {
        let said = said.lock().expect("the lines said");
        said.iter().any(|line| line == "/dev/null") && said.contains(&log)
    });

    let mut asking = session.stdin.take().expect("its stdin");
    for message in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "perspicax wire test", "version": "0" },
        }}),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "window_list" } }),
    ] {
        writeln!(asking, "{message}").expect("perspicax is listening");
    }
    for id in [1, 2] {
        let line = answers.recv_timeout(PATIENCE).expect("an answer");
        let answer: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("not JSON on the wire ({error}): {line}"));
        assert_eq!(answer["jsonrpc"], "2.0", "{line}");
        assert_eq!(answer["id"], id, "{line}");
    }

    // The agent leaves: the run ends, and the wire with it, having carried
    // nothing more. Closed, too, which it would not be while a program the
    // run started still held it.
    drop(asking);
    let status = exited(&mut session, PATIENCE);
    assert!(status.success(), "{status}");
    match answers.recv_timeout(PATIENCE) {
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        other => panic!("the wire carried more, or never closed: {other:?}"),
    }
}

/// Everything `from` carries until it closes, read on a thread of its own so
/// that a pipe something still holds fails the test rather than hanging it.
fn everything(mut from: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tell, said) = mpsc::channel();
    std::thread::spawn(move || {
        let mut all = String::new();
        let _ = from.read_to_string(&mut all);
        let _ = tell.send(all);
    });
    said
}

/// The agent's first word.
fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "perspicax wire test", "version": "0" },
    }})
}

/// `dbus-run-session -- perspicax --mcp` gives the bus daemon the wire before
/// perspicax runs, and the daemon gives it to every service it starts, where
/// perspicax cannot take it back (issue #52). So the run is refused before
/// anything could ask the bus to start something, saying how to keep the wire
/// from the bus.
#[test]
#[ignore = "starts a session bus of its own"]
fn a_wire_the_session_bus_holds_too_is_refused_before_anything_starts() {
    let mut session = Command::new("dbus-run-session")
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_perspicax"))
        .args(["--headless", "--mcp", "--run-for", "5"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dbus-run-session, which a session with a bus has");
    let wire = everything(session.stdout.take().expect("its stdout"));
    let said = everything(session.stderr.take().expect("its stderr"));
    // Asked, as an agent would, though a refused run may be gone already.
    let mut asking = session.stdin.take().expect("its stdin");
    let _ = writeln!(asking, "{}", initialize());
    drop(asking);

    let status = exited(&mut session, PATIENCE);
    let said = said.recv_timeout(PATIENCE).expect("its stderr closes");
    assert!(!status.success(), "{said}");
    for named in ["session bus", "pid ", "3<&0 4>&1", "--mcp-socket"] {
        assert!(said.contains(named), "the refusal says {named:?}: {said}");
    }
    assert_eq!(
        wire.recv_timeout(PATIENCE).expect("the wire closes"),
        "",
        "the wire carried something"
    );
}

/// The way the refusal above says to keep the wire from the bus: given to
/// perspicax alone, on fds 3 and 4, which `dbus-run-session` does not pass on
/// to its daemon. Served, and closed once perspicax has gone, which it would
/// not be while the bus or a service it started still held it.
#[test]
#[ignore = "needs a live accessibility bus"]
fn a_wire_kept_from_the_session_bus_is_served() {
    let mut session = Command::new("sh")
        .args([
            "-c",
            r#"exec dbus-run-session -- sh -c 'exec "$0" --headless --mcp <&3 >&4 3<&- 4>&-' "$1" 3<&0 4>&1 </dev/null >&2"#,
            "sh",
            env!("CARGO_BIN_EXE_perspicax"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sh");
    let said = everything(session.stderr.take().expect("its stderr"));
    let said = move || {
        said.recv_timeout(Duration::from_secs(1))
            .unwrap_or_default()
    };
    let (tell, answers) = mpsc::channel::<String>();
    let stdout = session.stdout.take().expect("its stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tell.send(line).is_err() {
                break;
            }
        }
    });

    let mut asking = session.stdin.take().expect("its stdin");
    writeln!(asking, "{}", initialize()).expect("perspicax is listening");
    writeln!(
        asking,
        "{}",
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .expect("perspicax is listening");
    let Ok(line) = answers.recv_timeout(PATIENCE) else {
        let _ = session.kill();
        panic!("no answer to initialize: {}", said());
    };
    let answer: Value = serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("not JSON on the wire ({error}): {line}"));
    assert_eq!(answer["id"], 1, "{line}");
    assert_eq!(
        answer["result"]["serverInfo"]["name"], "perspicax",
        "{line}"
    );

    drop(asking);
    let status = exited(&mut session, PATIENCE);
    assert!(status.success(), "{status}");
    match answers.recv_timeout(PATIENCE) {
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        other => panic!("the wire carried more, or never closed: {other:?}"),
    }
}
