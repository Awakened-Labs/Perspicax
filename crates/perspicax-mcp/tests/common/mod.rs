//! What the wire tests share: a desktop with nothing on it, and a client that
//! speaks JSON-RPC over whatever byte stream a test hands it -- a
//! `tokio::io::duplex` pair, or a Unix socket.
//!
//! Each test file compiles this module on its own and uses part of it, hence
//! the `dead_code` allowance.

#![allow(dead_code, reason = "each test file uses part of this module")]

use std::sync::Mutex;
use std::time::Duration;

use perspicax_index::{Delta, HostFacts, Index, Receipt, Refusal, Selector, Verb};
use perspicax_mcp::{Denied, Desktop};
use perspicax_node::NodeId;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};

/// How long any one exchange gets before the test gives up on it.
///
/// A bound rather than patience: a server that never answers should fail a
/// test in a few seconds, not hold a CI runner until something kills it with no
/// output to show for the wait.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// A desktop with nothing on it, which is what the server has to be able to
/// serve before any compositor has run or any accessibility bus been read.
///
/// Its one piece of state is the queue `deltas` drains, so that a test can say
/// something changed and see who is told.
#[derive(Default)]
pub struct Nothing {
    pending: Mutex<Vec<Delta>>,
}

impl Nothing {
    /// Something changes: one delta, pending.
    pub fn change(&self, id: u64) {
        self.queue().push(Delta::Added { id: NodeId(id) });
    }

    /// How many changes are waiting to be drained.
    pub fn pending(&self) -> usize {
        self.queue().len()
    }

    fn queue(&self) -> std::sync::MutexGuard<'_, Vec<Delta>> {
        self.pending.lock().expect("no test poisons this")
    }
}

impl Desktop for Nothing {
    fn read(&self, visit: &mut dyn FnMut(&Index, &HostFacts)) {
        visit(&Index::new(), &HostFacts::default());
    }

    fn deltas(&self) -> Vec<Delta> {
        std::mem::take(&mut *self.queue())
    }

    fn act(&self, _selector: &Selector, _verb: &Verb) -> Result<Receipt, Denied> {
        Err(Denied::Refused(Refusal::NotFound))
    }

    fn act_window(
        &self,
        _surface: perspicax_node::SurfaceId,
        _verb: perspicax_index::WindowVerb,
    ) -> Result<perspicax_index::WindowReceipt, Denied> {
        Err(Denied::Refused(Refusal::NotFound))
    }

    fn capture(
        &self,
        _target: perspicax_index::ShotTarget,
    ) -> Result<perspicax_index::Shot, Denied> {
        Err(Denied::NotBuilt("capture".to_owned()))
    }
}

/// One side of the wire, with the framing JSON-RPC over a byte stream uses.
pub struct Client<R, W> {
    lines: BufReader<R>,
    out: W,
}

impl<R, W> Client<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// A client reading the server's frames from `read` and writing its own to
    /// `write`.
    pub fn new(read: R, write: W) -> Self {
        Self {
            lines: BufReader::new(read),
            out: write,
        }
    }

    pub async fn send(&mut self, message: &Value) {
        let mut frame = message.to_string();
        frame.push('\n');
        self.out
            .write_all(frame.as_bytes())
            .await
            .expect("the server is listening");
        self.out.flush().await.expect("the server is listening");
    }

    pub async fn recv(&mut self) -> Value {
        let line = self.line().await;
        assert!(!line.is_empty(), "the server closed the connection");
        serde_json::from_str(&line).expect("the server writes JSON")
    }

    /// Whether the server has closed its side, having said nothing more.
    pub async fn closed(&mut self) -> bool {
        self.line().await.is_empty()
    }

    async fn line(&mut self) -> String {
        let mut line = String::new();
        tokio::time::timeout(PATIENCE, self.lines.read_line(&mut line))
            .await
            .expect("the server answered in time")
            .expect("the server wrote a frame");
        line
    }

    /// A request, and its answer. Notifications go through [`Client::send`].
    pub async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        let response = self.ask(id, method, params).await;
        assert!(
            response.get("error").is_none(),
            "the server refused the request: {response}"
        );
        response["result"].clone()
    }

    /// A request the server is expected to refuse, and its error.
    pub async fn refused(&mut self, id: u64, method: &str, params: Value) -> Value {
        let response = self.ask(id, method, params).await;
        assert!(
            response.get("result").is_none(),
            "the server answered a request it should have refused: {response}"
        );
        response["error"].clone()
    }

    async fn ask(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await;
        let response = self.recv().await;
        assert_eq!(response["id"], id, "answered the wrong request");
        response
    }

    /// A handshake, and what the server said about itself in it.
    pub async fn handshake(&mut self) -> Value {
        let result = self.call(1, "initialize", initialize()).await;
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        result
    }

    /// How many windows the server says there are.
    pub async fn windows(&mut self, id: u64) -> u64 {
        let windows = self
            .call(id, "tools/call", json!({ "name": "window_list" }))
            .await;
        windows["structuredContent"]["count"]
            .as_u64()
            .expect("window_list counts")
    }

    /// How many changes the server says there have been since this
    /// conversation last asked.
    pub async fn deltas(&mut self, id: u64) -> u64 {
        let changed = self
            .call(id, "tools/call", json!({ "name": "deltas" }))
            .await;
        changed["structuredContent"]["count"]
            .as_u64()
            .expect("deltas counts")
    }
}

/// What `initialize` carries.
pub fn initialize() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "perspicax-mcp wire test", "version": "0" },
    })
}
