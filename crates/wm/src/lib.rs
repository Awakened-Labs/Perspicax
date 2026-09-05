//! The composition root, as a library so a test can drive it.
//!
//! `wm` is a binary: it wires a compositor, an accessibility ingest and (from
//! M3) an MCP server into one process. The library half exists because the
//! milestone's claim -- *a node covered by another window is refused, and the
//! refusal names the surface in the way* -- is worth asserting automatically,
//! and an integration test cannot reach inside a binary crate.
//!
//! So the demo and the test run the same code, which is the only arrangement
//! in which a green test means the demo works.

pub mod observe;
pub mod session;
