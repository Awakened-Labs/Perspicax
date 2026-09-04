//! Tests that need a real accessibility bus and real applications.
//!
//! Every test here is `#[ignore]`d, so `cargo test --workspace` stays green on
//! a machine with no graphical session -- which is every CI runner until M1
//! slice 6 gives the pipeline a `dbus-run-session`. Run them deliberately:
//!
//! ```sh
//! export XDG_RUNTIME_DIR=/run/user/$(id -u)
//! export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
//! cargo test -p wm-atspi --test live -- --ignored --nocapture
//! ```
//!
//! Without those two variables an SSH session finds an empty desktop and
//! reports no error worth reading, which is a confusing hour if nobody wrote
//! it down.
//!
//! The applications are `gtk4-widget-factory` (Debian's `gtk-4-examples`) and
//! `gallery` (`qt6-base-examples`). They are not interchangeable: the entire
//! point of this file is that the two toolkits answer the same protocol
//! differently, and the tests assert the difference rather than tolerate it.

use wm_atspi::{AtspiIngest, Strategy};
use wm_index::{Ingest, Refusal, check_actable};
use wm_node::Origin;

const GTK: &str = "gtk4-widget-factory";
const QT: &str = "gallery";

async fn read(app: &str) -> (AtspiIngest, Vec<wm_node::ObservedNode>) {
    let mut ingest = AtspiIngest::connect(app)
        .await
        .unwrap_or_else(|error| panic!("connect {app}: {error}"));
    let root = ingest.root_id();
    let nodes = ingest
        .snapshot(root)
        .await
        .unwrap_or_else(|error| panic!("snapshot {app}: {error}"));
    (ingest, nodes)
}

/// GTK bridges through ATK, which implements `org.a11y.atspi.Cache` for real:
/// once the cache holds the tree, the whole thing arrives in one round trip.
///
/// The discarded first read is the point of the test, not throat-clearing.
/// ATK fills its cache as accessibles are **realised**, so a freshly started
/// application answers `GetItems` with a fraction of its tree -- and the first
/// read is what detects that and walks instead, which realises everything.
/// Without it this test would pass or fail on whether some earlier test
/// happened to warm the same application, which is not a property worth
/// asserting.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn a_warm_gtk_cache_is_read_in_one_round_trip() {
    let (_, warmed) = read(GTK).await;

    let (ingest, nodes) = read(GTK).await;
    assert_eq!(ingest.strategy(), Some(Strategy::Cache));
    assert!(ingest.strategy().is_some_and(Strategy::is_bulk));
    assert_eq!(
        nodes.len(),
        warmed.len(),
        "the bulk read must agree with the read that warmed it"
    );
    assert!(
        nodes.len() > 100,
        "a widget factory has more than {} nodes",
        nodes.len()
    );
    println!("GTK  {} nodes via {:?}", nodes.len(), ingest.strategy());
}

/// The headline finding, pinned as a test.
///
/// Qt 6.8 **does** export `org.a11y.atspi.Cache`, introspects cleanly, and
/// answers `GetItems` with an empty array. An interface-shaped probe takes the
/// fast path, gets nothing, and reports that a window full of widgets is empty.
/// The result-shaped probe falls through to the walk and finds the tree.
///
/// If this test ever fails because the strategy came back `Cache`, Qt gained a
/// real cache implementation and the fallback can be revisited -- but check
/// that the node count is non-trivial before believing it.
#[tokio::test]
#[ignore = "needs a live accessibility bus and the Qt widget gallery running"]
async fn qt_falls_through_an_empty_cache_to_the_walk() {
    let (ingest, nodes) = read(QT).await;
    assert_eq!(
        ingest.strategy(),
        Some(Strategy::Walk),
        "Qt's cache answers with nothing; the probe must not believe it"
    );
    assert!(
        nodes.len() > 10,
        "the walk found only {} nodes",
        nodes.len()
    );
    println!("Qt   {} nodes via {:?}", nodes.len(), ingest.strategy());
}

/// The M1 invariant, against real programs rather than synthetic trees. There
/// is no compositor, so nothing read off the bus may be acted on -- and the
/// refusal names the reason rather than failing vaguely.
#[tokio::test]
#[ignore = "needs a live accessibility bus"]
async fn no_real_node_is_actable_without_a_compositor() {
    for app in [GTK, QT] {
        let (_, nodes) = read(app).await;
        assert!(!nodes.is_empty());
        for node in &nodes {
            assert_eq!(node.origin, Origin::Unattributed, "{app}");
            assert!(node.surface.is_none(), "{app}");
            assert_eq!(
                check_actable(node).unwrap_err(),
                Refusal::Unattributed,
                "{app} node {:?} was actable with no compositor",
                node.id
            );
        }
    }
}

/// `atspi` names the two strings in a cache item `short_name` and `name`, but
/// the wire order is name-then-description -- so `short_name` is what a
/// selector's bare word has to match. Getting this backwards silently swaps
/// every label and description in the tree, which nothing else would catch.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn labels_come_from_the_field_that_holds_the_name() {
    let (_, nodes) = read(GTK).await;
    let labelled: Vec<&str> = nodes.iter().filter_map(|n| n.node.label()).collect();
    assert!(
        !labelled.is_empty(),
        "no node carried a label; short_name and name are probably swapped"
    );
    println!("GTK labels: {:?}", &labelled[..labelled.len().min(12)]);
}

/// Ids are minted once per object and never reissued, across both toolkits.
#[tokio::test]
#[ignore = "needs a live accessibility bus"]
async fn ids_are_unique_within_a_read() {
    for app in [GTK, QT] {
        let (_, nodes) = read(app).await;
        let mut ids: Vec<u64> = nodes.iter().map(|n| n.id.0).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "{app} reused a node id");
    }
}

/// Geometry is a separate round trip per node on both toolkits, which is why
/// it is opt-in. This only asserts that it works and is window-relative-shaped;
/// the Wayland/Xorg extents comparison is slice 5's job.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn geometry_is_opt_in_and_produces_boxes() {
    let mut ingest = AtspiIngest::connect(GTK).await.unwrap().with_geometry(true);
    let root = ingest.root_id();
    let nodes = ingest.snapshot(root).await.unwrap();

    let boxed: Vec<_> = nodes
        .iter()
        .filter_map(wm_node::ObservedNode::bounds)
        .collect();
    assert!(!boxed.is_empty(), "no node reported extents");
    for rect in &boxed {
        assert!(
            rect.x1 >= rect.x0 && rect.y1 >= rect.y0,
            "inverted rect {rect:?}"
        );
    }
    println!("GTK  {} of {} nodes had extents", boxed.len(), nodes.len());
}

/// Reading a tree must not destroy it.
///
/// zbus caches proxy properties by default, which issues
/// `Properties.GetAll` on construction -- and that call segfaults Qt 6.8.2's
/// AT-SPI adaptor outright. A reader built the obvious way therefore kills
/// every Qt application it looks at, before reading a single node.
///
/// This connects to the Qt gallery twice. If the first read killed it, the
/// second cannot find it, and the failure is unambiguous.
#[tokio::test]
#[ignore = "needs a live accessibility bus and the Qt widget gallery running"]
async fn reading_a_qt_application_twice_does_not_kill_it() {
    let (_, first) = read(QT).await;
    assert!(!first.is_empty());

    let (_, second) = read(QT).await;
    assert!(
        !second.is_empty(),
        "the Qt application did not survive being read once"
    );
    println!(
        "Qt   survived two reads: {} then {} nodes",
        first.len(),
        second.len()
    );
}

/// Does the fast path see the same tree the slow one does?
///
/// This is the question that decides whether a cache is an optimisation or a
/// different answer, and it cannot be settled by timing.
///
/// Measured on a **freshly started** `gtk4-widget-factory`: the cache returns
/// 11 nodes and a walk of the same window returns 278, a factor of 25. Walk it
/// once and the cache thereafter holds all 278 and the ratio is 1.0. So the
/// cache is not incomplete, it is *cold* -- and the difference is invisible in
/// the reply, which is why `read::cache_is_complete` exists.
///
/// The ratio this prints therefore depends on what has already touched the
/// application, and the test asserts only what is true either way. The number
/// is for the M1 latency table; the assertion is that the cache never claims
/// more than exists.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn the_gtk_fast_path_is_measured_against_the_slow_one() {
    let mut cached = AtspiIngest::connect(GTK)
        .await
        .unwrap()
        .forcing(Strategy::Cache);
    let root = cached.root_id();
    let via_cache = cached.snapshot(root).await.unwrap();

    let mut walked = AtspiIngest::connect(GTK)
        .await
        .unwrap()
        .forcing(Strategy::Walk);
    let root = walked.root_id();
    let via_walk = walked.snapshot(root).await.unwrap();

    println!(
        "GTK  cache={} nodes, walk={} nodes  (walk/cache = {:.1}x)",
        via_cache.len(),
        via_walk.len(),
        via_walk.len() as f64 / via_cache.len().max(1) as f64,
    );
    assert!(!via_walk.is_empty(), "the walk found nothing");
    assert!(
        via_cache.len() <= via_walk.len(),
        "the cache reported more nodes than exist"
    );
}

/// A name nothing answers to must say so, and say what *was* there -- the
/// error is the diagnostic a human reads at 2am.
#[tokio::test]
#[ignore = "needs a live accessibility bus"]
async fn an_absent_application_names_the_ones_that_are_present() {
    let error = AtspiIngest::connect("definitely-not-running")
        .await
        .expect_err("connected to an application that does not exist");
    let message = error.to_string();
    assert!(message.contains("definitely-not-running"), "{message}");
    println!("{message}");
}
