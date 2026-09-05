//! The server, over a real transport, with no compositor behind it at all.
//!
//! This is slice 4's exit criterion stated as a test: *the server answers
//! `initialize` and `tools/list` over stdio with no compositor running*. It
//! speaks the wire rather than calling the tool methods -- the unit tests
//! already do that -- because everything between a method and a frame is
//! generated code, and generated code is exactly what a hand-written call
//! cannot exercise.
//!
//! The transport is a `tokio::io::duplex` pair rather than this process's
//! stdio, for the obvious reason and a less obvious one: `cargo test` owns
//! stdout, so a server writing frames to it would interleave with the harness
//! -- the same failure mode `--mcp` has with `tracing`, and one worth not
//! demonstrating twice.
//!
//! Implementing [`Desktop`] out here also proves the trait is satisfiable from
//! outside the crate, which is the whole claim of it: a GNOME extension or a
//! KWin plugin re-implements these three methods and gets the six tools.

use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt as _;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use wm_index::{Delta, HostFacts, Index, Receipt, Refusal, Selector, Verb};
use wm_mcp::{Denied, Desktop, Wm};

/// How long any one exchange gets before the test gives up on it.
///
/// A bound rather than patience: a server that never answers should fail this
/// test in a few seconds, not hold a CI runner until something kills it with no
/// output to show for the wait.
const PATIENCE: Duration = Duration::from_secs(5);

/// A desktop with nothing on it, which is exactly what this slice claims to be
/// able to serve. No compositor has run, no accessibility bus has been read,
/// and every tool still has to answer.
struct Nothing;

impl Desktop for Nothing {
    fn read(&self, visit: &mut dyn FnMut(&Index, &HostFacts)) {
        visit(&Index::new(), &HostFacts::default());
    }

    fn deltas(&self) -> Vec<Delta> {
        Vec::new()
    }

    fn act(&self, _selector: &Selector, _verb: &Verb) -> Result<Receipt, Denied> {
        Err(Denied::Refused(Refusal::NotFound))
    }
}

/// One side of the wire, with the framing JSON-RPC over a byte stream uses.
struct Client {
    lines: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    out: tokio::io::WriteHalf<tokio::io::DuplexStream>,
}

impl Client {
    async fn send(&mut self, message: &Value) {
        let mut frame = message.to_string();
        frame.push('\n');
        self.out
            .write_all(frame.as_bytes())
            .await
            .expect("the server is listening");
        self.out.flush().await.expect("the server is listening");
    }

    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        let read = tokio::time::timeout(PATIENCE, self.lines.read_line(&mut line))
            .await
            .expect("the server answered in time")
            .expect("the server wrote a frame");
        assert!(read > 0, "the server closed the connection");
        serde_json::from_str(&line).expect("the server writes JSON")
    }

    /// A request, and its answer. Notifications go through [`Client::send`].
    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await;
        let response = self.recv().await;
        assert_eq!(response["id"], id, "answered the wrong request");
        assert!(
            response.get("error").is_none(),
            "the server refused the request: {response}"
        );
        response["result"].clone()
    }
}

/// A server on one end of a duplex pair, and a client on the other.
fn connected() -> (Client, tokio::task::JoinHandle<()>) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let (client_read, client_write) = tokio::io::split(client);

    let serving = tokio::spawn(async move {
        let service = Wm::new(Arc::new(Nothing))
            .serve((server_read, server_write))
            .await
            .expect("the transport is a pair of pipes");
        // The client dropping its end is an ordinary end to the conversation
        // and not a failure, so the result is deliberately not unwrapped.
        let _ = service.waiting().await;
    });

    (
        Client {
            lines: BufReader::new(client_read),
            out: client_write,
        },
        serving,
    )
}

/// A handshake, and what the server said about itself in it.
async fn handshake(client: &mut Client) -> Value {
    let result = client
        .call(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "wm-mcp protocol test", "version": "0" },
            }),
        )
        .await;
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    result
}

#[tokio::test]
async fn the_server_introduces_itself_and_lists_its_tools_with_no_compositor_running() {
    let (mut client, serving) = connected();

    let hello = handshake(&mut client).await;
    assert_eq!(hello["serverInfo"]["name"], "wm");
    assert!(hello["capabilities"]["tools"].is_object());
    let instructions = hello["instructions"]
        .as_str()
        .expect("the server introduces itself");
    assert!(instructions.contains("untrusted_text"));

    let listed = client.call(2, "tools/list", json!({})).await;
    let tools = listed["tools"].as_array().expect("a list of tools");
    let mut names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("each tool has a name"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "act",
            "deltas",
            "observe",
            "resolve",
            "screenshot",
            "window_list"
        ]
    );

    // A description a model can choose on, and a schema it can fill in. Both
    // are the tool as far as a client is concerned.
    for tool in tools {
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| text.len() > 200),
            "{} has no description worth reading",
            tool["name"]
        );
        assert!(tool["inputSchema"].is_object(), "{}", tool["name"]);
    }

    drop(client);
    serving.await.expect("the server stopped cleanly");
}

#[tokio::test]
async fn the_tools_answer_over_the_wire_and_refusals_arrive_as_refusals() {
    let (mut client, serving) = connected();
    handshake(&mut client).await;

    // An empty desktop is an answer, not an error: no windows, no nodes.
    let windows = client
        .call(2, "tools/call", json!({ "name": "window_list" }))
        .await;
    assert_eq!(windows["structuredContent"]["count"], 0);
    assert_eq!(windows["isError"], false);

    // The one tool that ships refusing, refusing.
    let picture = client
        .call(3, "tools/call", json!({ "name": "screenshot" }))
        .await;
    assert_eq!(picture["isError"], true);
    assert_eq!(picture["structuredContent"]["reason"], "no_renderer");
    assert_eq!(picture["structuredContent"]["unexplained"], 0);

    // A selector that matches nothing on an empty desktop: refused by name,
    // and as a tool result rather than a transport failure.
    let acted = client
        .call(
            4,
            "tools/call",
            json!({
                "name": "act",
                "arguments": { "selector": "button:Cancel", "verb": "click" },
            }),
        )
        .await;
    assert_eq!(acted["isError"], true);
    assert_eq!(acted["structuredContent"]["kind"], "not_found");

    // And a malformed request is a protocol error rather than a tool result,
    // which is the distinction `wm-index` draws between a selector that is not
    // one and a screen that did not satisfy a perfectly good one.
    client
        .send(&json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "act",
                "arguments": { "selector": "button:Close[x]", "verb": "click" },
            },
        }))
        .await;
    let error = client.recv().await;
    assert_eq!(error["id"], 5);
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("is not a number")),
        "{error}"
    );

    drop(client);
    serving.await.expect("the server stopped cleanly");
}
