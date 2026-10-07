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
//! KWin plugin re-implements these five methods and gets the eight tools.

mod common;

use std::sync::Arc;

use common::{Client, Nothing, PATIENCE};
use perspicax_mcp::ServeError;
use serde_json::json;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

/// The client's end of a duplex pair.
type Duplex = Client<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;

/// How the server's side of a conversation ended.
type Serving = tokio::task::JoinHandle<Result<(), ServeError>>;

/// A server on one end of a duplex pair, and a client on the other.
///
/// Served by [`perspicax_mcp::serve_over`], which is what `--mcp` runs over
/// stdio, so every exchange here goes through the same door a real client's
/// does -- the gate in front of the handshake included.
fn connected() -> (Duplex, Serving) {
    connected_to(Arc::new(Nothing::default()))
}

/// [`connected`], to a desktop the test keeps a hand on.
fn connected_to(desktop: Arc<Nothing>) -> (Duplex, Serving) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let (client_read, client_write) = tokio::io::split(client);

    let serving = tokio::spawn(perspicax_mcp::serve_over(
        desktop,
        (server_read, server_write),
    ));

    (Client::new(client_read, client_write), serving)
}

/// The client has gone; the server should have taken that as an ordinary end.
async fn ended_cleanly(serving: Serving) {
    tokio::time::timeout(PATIENCE, serving)
        .await
        .expect("the server noticed the client go")
        .expect("the server did not panic")
        .expect("a client going away is an ordinary end");
}

#[tokio::test]
async fn the_server_introduces_itself_and_lists_its_tools_with_no_compositor_running() {
    let (mut client, serving) = connected();

    let hello = client.handshake().await;
    assert_eq!(hello["serverInfo"]["name"], "perspicax");
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
            "tab_forward",
            "window_close",
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
    ended_cleanly(serving).await;
}

#[tokio::test]
async fn the_tools_answer_over_the_wire_and_refusals_arrive_as_refusals() {
    let (mut client, serving) = connected();
    client.handshake().await;

    // An empty desktop is an answer, not an error: no windows, no nodes.
    let windows = client
        .call(2, "tools/call", json!({ "name": "window_list" }))
        .await;
    assert_eq!(windows["structuredContent"]["count"], 0);
    assert_eq!(windows["isError"], false);

    // A desktop that cannot take pictures says so, and why.
    let picture = client
        .call(3, "tools/call", json!({ "name": "screenshot" }))
        .await;
    assert_eq!(picture["isError"], true);
    assert_eq!(picture["structuredContent"]["reason"], "not_built");
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
    // which is the distinction `perspicax-index` draws between a selector that
    // is not one and a screen that did not satisfy a perfectly good one.
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
    ended_cleanly(serving).await;
}

// Issue #26. A client that speaks before its handshake -- one restarted
// without it, one retrying, one simply wrong -- used to end the transport, and
// `perspicax` ended the session with it: on a seat, every window the person
// had open. One mis-ordered message must cost that message, not the desktop.

#[tokio::test]
async fn a_request_before_the_handshake_is_refused_and_the_handshake_still_works() {
    let (mut client, serving) = connected();

    client
        .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }))
        .await;
    let refused = client.recv().await;
    assert_eq!(refused["id"], 1, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("initialize")),
        "the refusal should say what to send first: {refused}"
    );

    // And the conversation is still there to be had.
    client.handshake().await;
    let listed = client.call(2, "tools/list", json!({})).await;
    assert_eq!(listed["tools"].as_array().map(Vec::len), Some(8));

    drop(client);
    ended_cleanly(serving).await;
}

#[tokio::test]
async fn a_notification_before_the_handshake_is_ignored() {
    let (mut client, serving) = connected();

    // Sent too early, as a client that lost track of its own state might.
    client
        .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;

    client.handshake().await;
    let windows = client
        .call(2, "tools/call", json!({ "name": "window_list" }))
        .await;
    assert_eq!(windows["structuredContent"]["count"], 0);

    drop(client);
    ended_cleanly(serving).await;
}

#[tokio::test]
async fn a_ping_before_the_handshake_is_still_answered() {
    let (mut client, serving) = connected();

    // MCP allows this one, and the gate must not take it away.
    let pong = client.call(7, "ping", json!({})).await;
    assert_eq!(pong, json!({}));

    client.handshake().await;
    let listed = client.call(2, "tools/list", json!({})).await;
    assert_eq!(listed["tools"].as_array().map(Vec::len), Some(8));

    drop(client);
    ended_cleanly(serving).await;
}

#[tokio::test]
async fn a_client_that_leaves_before_its_handshake_is_an_ordinary_end() {
    let (client, serving) = connected();
    drop(client);
    ended_cleanly(serving).await;
}

#[tokio::test]
async fn a_conversations_deltas_begin_when_it_does() {
    let desktop = Arc::new(Nothing::default());
    desktop.change(1);
    let (mut client, serving) = connected_to(Arc::clone(&desktop));

    // Nothing was said to this conversation before it opened, so nothing from
    // then is owed to it.
    client.handshake().await;
    assert_eq!(client.deltas(2).await, 0);

    desktop.change(2);
    assert_eq!(client.deltas(3).await, 1);
    assert_eq!(client.deltas(4).await, 0, "a change is reported once");

    drop(client);
    ended_cleanly(serving).await;
}
