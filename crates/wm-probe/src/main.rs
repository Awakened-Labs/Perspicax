//! `wm-probe` -- the tool that makes a milestone demonstrable.
//!
//! `dump` prints a semantic tree, `time` records cold-read and warm-delta
//! latency, `explain` says why a node was refused. The latency numbers are not
//! decoration: the case for ever building a faster ingest path rests on them,
//! so they get measured from M1 rather than asserted.

fn main() {
    println!(
        "wm-probe {} -- no ingest path yet; `dump`, `time` and `explain` arrive with M1.",
        env!("CARGO_PKG_VERSION")
    );
}
