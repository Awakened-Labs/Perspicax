//! `perspicax --mcp-socket`, the binary itself: what it refuses before it
//! starts anything, `perspicax attach` carrying a conversation, and a headless
//! session serving one conversation after another while it runs on (issue #30).
//!
//! The refusals need nothing but a filesystem, because the socket is bound
//! before the accessibility registry is touched, and `attach` is tested against
//! a socket this test serves over a desk with no compositor behind it; all of
//! them run wherever the tests do. The session needs a live accessibility bus, so it is `#[ignore]`d,
//! and `ci/live-tests.sh` runs it with `--include-ignored`.

use std::{
    fs,
    io::{BufRead as _, BufReader, Write as _},
    os::unix::{fs::PermissionsExt as _, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::Arc,
    sync::atomic::{AtomicUsize, Ordering},
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
