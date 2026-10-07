//! The agent interface on a Unix socket: the file, and conversations over it.
//!
//! Real sockets in a scratch directory, and no compositor: everything issue
//! #30 asks of the socket is a property of this crate, so it is tested here,
//! where it runs on any machine CI does. Every client is this process, so the
//! pid the server names for whoever holds the conversation is this test's own.

mod common;

use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{fs, os::unix::net::UnixListener};

use common::{Client, Nothing, PATIENCE, initialize};
use perspicax_mcp::{Socket, SocketError};
use serde_json::json;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// A directory of this test's own, emptied and removed when dropped.
///
/// Short on purpose: a socket's path has to fit in 108 bytes.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pxs-{}-{}",
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

/// A socket being served, as `--mcp-socket` serves one.
struct Served {
    path: PathBuf,
    desktop: Arc<Nothing>,
    serving: tokio::task::JoinHandle<()>,
    _socket: Socket,
}

impl Served {
    fn at(path: PathBuf) -> Self {
        let desktop = Arc::new(Nothing::default());
        let (socket, listener) = Socket::bind(&path).expect("a socket to serve");
        let serving = tokio::spawn({
            let desktop: Arc<dyn perspicax_mcp::Desktop> = desktop.clone();
            async move {
                let Err(error) = perspicax_mcp::serve_socket(desktop, listener).await;
                panic!("the socket stopped being served: {error}");
            }
        });
        Self {
            path,
            desktop,
            serving,
            _socket: socket,
        }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

type Unix = Client<OwnedReadHalf, OwnedWriteHalf>;

/// A connection, with nothing said on it yet.
async fn connect(path: &Path) -> Unix {
    let stream = UnixStream::connect(path).await.expect("the socket answers");
    let (read, write) = stream.into_split();
    Client::new(read, write)
}

/// A conversation, once whichever one came before has let go of the desktop.
///
/// A conversation ends when the server notices its client has gone, which is a
/// moment after the client goes, so a test that connects again straight away
/// may find the slot still held for that moment.
async fn converse(path: &Path) -> Unix {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let mut client = connect(path).await;
        client
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize() }))
            .await;
        let answer = client.recv().await;
        if answer.get("error").is_none() {
            client
                .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .await;
            return client;
        }
        assert!(
            Instant::now() < deadline,
            "the desktop was never free: {answer}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// The file.

#[test]
fn the_socket_is_its_owners_alone() {
    let scratch = Scratch::new();
    let (socket, _listener) = Socket::bind(scratch.join("s")).expect("a socket");

    let found = fs::symlink_metadata(socket.path()).expect("the socket is there");
    assert!(found.file_type().is_socket());
    assert_eq!(found.permissions().mode() & 0o777, 0o600);
    assert!(socket.path().is_absolute());
}

#[test]
fn a_socket_nothing_answers_on_is_replaced() {
    let scratch = Scratch::new();
    let path = scratch.join("s");
    // What a session killed before it could tidy up leaves behind.
    drop(UnixListener::bind(&path).expect("a socket"));
    assert!(path.exists());

    let (socket, _listener) = Socket::bind(&path).expect("a stale socket is replaced");
    assert_eq!(socket.path(), path);
}

#[test]
fn a_socket_something_answers_on_is_refused_naming_who_answers() {
    let scratch = Scratch::new();
    let path = scratch.join("s");
    let (_first, _listener) = Socket::bind(&path).expect("a socket");

    let error = Socket::bind(&path).expect_err("the path is taken");
    let me = std::process::id();
    assert!(
        matches!(error, SocketError::Live { pid: Some(pid), .. } if pid == me),
        "{error:?}"
    );
    assert!(error.to_string().contains(&format!("pid {me}")), "{error}");
}

#[test]
fn what_is_not_a_socket_is_left_alone() {
    let scratch = Scratch::new();
    let file = scratch.join("f");
    fs::write(&file, "somebody's").expect("a file");
    let link = scratch.join("l");
    std::os::unix::fs::symlink(scratch.join("elsewhere"), &link).expect("a link");

    for taken in [&file, &link] {
        let error = Socket::bind(taken).expect_err("not a socket");
        assert!(
            matches!(error, SocketError::NotASocket { .. }),
            "{taken:?}: {error:?}"
        );
    }
    assert_eq!(
        fs::read_to_string(&file).expect("still there"),
        "somebody's"
    );
    assert!(fs::symlink_metadata(&link).is_ok_and(|found| found.is_symlink()));
}

#[test]
fn the_socket_goes_when_it_is_dropped_and_not_one_that_replaced_it() {
    let scratch = Scratch::new();
    let path = scratch.join("s");

    let (socket, listener) = Socket::bind(&path).expect("a socket");
    drop((socket, listener));
    assert!(!path.exists(), "dropped, and still there");

    // Somebody removes this one's file, and another socket takes the path.
    let (first, _first_listener) = Socket::bind(&path).expect("a socket");
    fs::remove_file(&path).expect("removed from under it");
    let (second, _second_listener) = Socket::bind(&path).expect("the path is free again");
    drop(first);
    assert!(path.exists(), "the first took the second's file with it");
    drop(second);
    assert!(!path.exists());
}

// Conversations over it.

#[tokio::test]
async fn a_conversation_ending_leaves_the_socket_serving_the_next() {
    let scratch = Scratch::new();
    let served = Served::at(scratch.join("s"));

    let mut first = connect(&served.path).await;
    first.handshake().await;
    assert_eq!(first.windows(2).await, 0);
    served.desktop.change(1);
    drop(first);

    let mut second = converse(&served.path).await;
    assert_eq!(second.windows(2).await, 0);
    assert_eq!(
        second.deltas(3).await,
        0,
        "what changed before this conversation is not owed to it"
    );
}

#[tokio::test]
async fn while_one_conversation_is_open_another_is_refused_and_the_first_goes_on() {
    let scratch = Scratch::new();
    let served = Served::at(scratch.join("s"));
    let mut first = connect(&served.path).await;
    first.handshake().await;

    let mut second = connect(&served.path).await;
    let refused = second.refused(1, "initialize", initialize()).await;
    let reason = refused["message"].as_str().expect("a reason");
    assert!(reason.contains("one at a time"), "{reason}");
    assert!(
        reason.contains(&format!("pid {}", std::process::id())),
        "the refusal names who holds the conversation: {reason}"
    );
    assert!(second.closed().await, "a refused connection is closed");

    assert_eq!(first.windows(2).await, 0, "the first conversation goes on");
    drop(first);
    let mut third = converse(&served.path).await;
    assert_eq!(third.windows(2).await, 0);
}

#[tokio::test]
async fn a_request_before_the_handshake_is_refused_on_its_connection_only() {
    let scratch = Scratch::new();
    let served = Served::at(scratch.join("s"));

    let mut early = connect(&served.path).await;
    let refused = early.refused(1, "tools/list", json!({})).await;
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("initialize")),
        "{refused}"
    );

    // It holds nothing, so the next connection opens at once.
    let mut next = connect(&served.path).await;
    next.handshake().await;
    assert_eq!(next.windows(2).await, 0);

    // And the early one, opening now, finds it taken.
    let refused = early.refused(2, "initialize", initialize()).await;
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("one at a time")),
        "{refused}"
    );
}

#[tokio::test]
async fn what_changes_while_nobody_is_connected_is_let_go_of() {
    let scratch = Scratch::new();
    let served = Served::at(scratch.join("s"));
    served.desktop.change(1);

    let deadline = Instant::now() + PATIENCE;
    while served.desktop.pending() > 0 {
        assert!(
            Instant::now() < deadline,
            "kept a change for nobody for {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn what_changes_during_a_conversation_is_kept_for_it() {
    let scratch = Scratch::new();
    let served = Served::at(scratch.join("s"));
    let mut client = connect(&served.path).await;
    client.handshake().await;

    served.desktop.change(1);
    // Longer than the server lets a change wait when nobody is connected.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(client.deltas(2).await, 1);
}
