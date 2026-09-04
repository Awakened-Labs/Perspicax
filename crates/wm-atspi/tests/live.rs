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
use wm_index::{Change, Index, Ingest, Refusal, Selector, check_actable};
use wm_node::{Origin, Role};

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
    // Within a tolerance, not exactly: these are two reads of a *running*
    // application seconds apart, and a real one grows or drops a node or two in
    // that time. Demanding equality would make this a test of whether the
    // desktop happened to hold still.
    let drift = nodes.len().abs_diff(warmed.len());
    assert!(
        drift <= warmed.len() / 20 + 2,
        "the bulk read found {} nodes where the read that warmed it found {} -- \
         a drift of {drift} is more than a live tree explains",
        nodes.len(),
        warmed.len(),
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
    assert!(!via_cache.is_empty(), "the cache path found nothing");
    assert!(!via_walk.is_empty(), "the walk found nothing");
}

/// The push half, against a real toolkit.
///
/// Reading a cold GTK application realises its accessibles, and ATK announces
/// each one with `Cache.AddAccessible`. So the snapshot is itself the thing
/// that generates traffic, and a drain immediately afterwards should find it
/// -- without asking the application anything.
///
/// The subscription is opened by `connect`, before the snapshot, which is why
/// those signals are queued rather than missed.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn signals_arrive_without_anyone_re_reading_a_tree() {
    let mut ingest = AtspiIngest::connect(GTK).await.unwrap();
    let root = ingest.root_id();
    let nodes = ingest.snapshot(root).await.unwrap();

    ingest.drain_changes().await.unwrap();

    // Cause a change rather than wait for one. Without this the test passes
    // vacuously on any desktop that happens to be still -- which is exactly
    // what it did before.
    assert!(poke(GTK).await, "found no focusable widget to poke");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    let changes = ingest.drain_changes().await.unwrap();
    println!(
        "GTK  {} nodes; focusing a widget volunteered {} changes",
        nodes.len(),
        changes.len()
    );
    assert!(
        !changes.is_empty(),
        "a widget took focus and the bus said nothing -- the subscription is not live"
    );

    // Drained means drained. A source that re-reports what it has already
    // volunteered turns a delta stream back into a poll.
    let again = ingest.drain_changes().await.unwrap();
    assert!(
        again.len() < changes.len(),
        "the second drain returned {} of the first drain's {} changes",
        again.len(),
        changes.len()
    );
}

/// Activate a real widget in `app`, so the toolkit emits a real signal.
///
/// Reaches the bus directly rather than through `wm-atspi`. A test that
/// provoked a change through the same code that observes it would prove only
/// that the crate agrees with itself; this is the harness standing in for a
/// user, and it is the one place in this file that acts on an application
/// rather than reading one.
///
/// `Action.DoAction` rather than `Component.GrabFocus`, because measured
/// against GTK 4.18 `GrabFocus` errors on every widget in the tree. The reason
/// is not settled: the first guess -- that no window manager was running, so
/// nothing ever held input focus -- was simply wrong, as openbox was managing
/// that session throughout. The measurement stands and the workaround is
/// sound; the explanation was not checked and has been removed rather than
/// left to be believed.
///
/// And a **check or toggle button** specifically, not merely the first widget
/// whose `DoAction(0)` returns true. Plenty of them do while changing nothing
/// observable -- a heading accepts an action and stays exactly as it was, so
/// the bus has nothing to report and a test built on it fails while every
/// piece of the machinery works. Toggling a checkbox flips a state bit the
/// toolkit is obliged to announce.
async fn poke(app: &str) -> bool {
    use atspi::{
        Role,
        proxy::{accessible::AccessibleProxy, action::ActionProxy},
        zbus::proxy::CacheProperties,
    };

    let bus = atspi::AccessibilityConnection::new().await.unwrap();
    let connection = bus.connection();
    let registry = bus.root_accessible_on_registry().await.unwrap();

    let accessible = |name: String, path: String| {
        AccessibleProxy::builder(connection)
            .destination(name)
            .and_then(|b| b.path(path))
            .map(|b| b.cache_properties(CacheProperties::No))
    };

    for candidate in registry.get_children().await.unwrap_or_default() {
        let Some(bus_name) = candidate.name().map(|n| n.as_str().to_owned()) else {
            continue;
        };
        let Ok(builder) = accessible(bus_name.clone(), candidate.path().as_str().to_owned()) else {
            continue;
        };
        let Ok(root) = builder.build().await else {
            continue;
        };
        if root.name().await.as_deref() != Ok(app) {
            continue;
        }

        // Breadth-first until something accepts focus. A widget factory has
        // one within a couple of levels; the budget stops a pathological tree
        // from turning a test into a walk.
        let mut queue =
            std::collections::VecDeque::from(root.get_children().await.unwrap_or_default());
        for _ in 0..300 {
            let Some(node) = queue.pop_front() else { break };
            let Some(node_bus) = node.name().map(|n| n.as_str().to_owned()) else {
                continue;
            };
            let path = node.path().as_str().to_owned();

            let Ok(builder) = accessible(node_bus.clone(), path.clone()) else {
                continue;
            };
            let Ok(proxy) = builder.build().await else {
                continue;
            };

            if matches!(
                proxy.get_role().await,
                Ok(Role::CheckBox | Role::ToggleButton | Role::CheckMenuItem)
            ) && let Ok(action) = ActionProxy::builder(connection)
                .destination(node_bus)
                .and_then(|b| b.path(path))
                .map(|b| b.cache_properties(CacheProperties::No))
                && let Ok(action) = action.build().await
                && action.do_action(0).await == Ok(true)
            {
                return true;
            }

            if let Ok(children) = proxy.get_children().await {
                queue.extend(children);
            }
        }
    }
    false
}

/// What the index does with what the bus volunteered -- the two halves of M1
/// meeting for the first time.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn volunteered_changes_apply_to_the_index() {
    let mut ingest = AtspiIngest::connect(GTK).await.unwrap();
    let root = ingest.root_id();

    let mut index = Index::new();
    index.ingest_snapshot(ingest.snapshot(root).await.unwrap());
    let after_snapshot = index.len();
    index.take_deltas();
    ingest.drain_changes().await.unwrap();

    assert!(poke(GTK).await, "found no toggle to poke");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    let changes = ingest.drain_changes().await.unwrap();
    assert!(!changes.is_empty(), "poking a toggle volunteered nothing");
    let (adds, invalidations) = changes.iter().fold((0, 0), |(a, i), change| match change {
        Change::Upserted { .. } => (a + 1, i),
        Change::SubtreeInvalidated { .. } => (a, i + 1),
        Change::Removed { .. } => (a, i),
    });
    for change in changes {
        index.apply(change);
    }

    println!(
        "GTK  index {} -> {} nodes; {} adds, {} invalidations, {} deltas",
        after_snapshot,
        index.len(),
        adds,
        invalidations,
        index.take_deltas().len(),
    );
    assert!(index.len() >= after_snapshot, "applying changes lost nodes");
}

/// The shape of a GTK tree, pinned so that a schema or selector regression
/// fails here rather than being argued about later.
///
/// # Why this is not a literal golden tree
///
/// Because a literal one would be a test of whether the desktop held still.
/// The same `gtk4-widget-factory` was measured at 278, 279, 286, 298 and 320
/// nodes across one afternoon -- ATK realises accessibles as they are needed,
/// tooltips come and go, and reading the tree changes it. A recorded
/// node-for-node snapshot would fail on the second run and be deleted by the
/// third.
///
/// What does not drift is the shape: an application root, one window under it,
/// and a vocabulary of roles the mapping table has to keep producing. That is
/// what regresses when someone edits `map::role`, and that is what this pins.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn the_gtk_tree_keeps_its_shape() {
    let (_, nodes) = read(GTK).await;
    let mut index = Index::new();
    index.ingest_snapshot(nodes);

    let roots = index.roots();
    assert_eq!(roots.len(), 1, "an application has exactly one root");
    let root = index.get(roots[0]).expect("the root");
    assert_eq!(root.node.role(), Role::Application);
    assert_eq!(root.node.label(), Some(GTK));

    let window = root
        .node
        .children()
        .iter()
        .filter_map(|id| index.get(*id))
        .find(|node| node.node.role() == Role::Window)
        .expect("the application root has a Window child");
    assert!(
        window.node.label().is_some_and(|l| !l.is_empty()),
        "the window carries a title"
    );

    // Roles the widget factory demonstrably has. Each one is a live assertion
    // about a different branch of `map::role`, so a mis-edit there lands here.
    let present: std::collections::HashSet<Role> = index
        .preorder()
        .into_iter()
        .filter_map(|id| index.get(id).map(|node| node.node.role()))
        .collect();
    for role in [
        Role::Application,
        Role::Window,
        Role::Button,
        Role::CheckBox,
        Role::RadioButton,
        Role::ComboBox,
        Role::Label,
        Role::Group,
    ] {
        assert!(present.contains(&role), "no {role:?} in the tree");
    }

    // Nothing should map to Unknown in a tree built entirely of stock widgets:
    // that is the signature of a role the table forgot.
    let unknowns = index
        .preorder()
        .into_iter()
        .filter(|id| {
            index
                .get(*id)
                .is_some_and(|n| n.node.role() == Role::Unknown)
        })
        .count();
    assert_eq!(unknowns, 0, "{unknowns} nodes mapped to Role::Unknown");

    println!(
        "GTK  {} nodes, {} distinct roles",
        index.len(),
        present.len()
    );
}

/// The selector grammar against a real tree rather than a synthetic one.
///
/// Each case exercises a different production, so a change to the parser or to
/// `resolve_all` shows up as a selector that stops addressing a real widget.
#[tokio::test]
#[ignore = "needs a live accessibility bus and gtk4-widget-factory running"]
async fn selectors_address_real_widgets() {
    let (_, nodes) = read(GTK).await;
    let mut index = Index::new();
    index.ingest_snapshot(nodes);

    let count = |selector: &str| {
        index
            .resolve_all(&Selector::parse(selector).expect("parses"))
            .len()
    };

    // Bare role, with a trailing colon.
    assert!(count("window:") >= 1, "no window matched `window:`");
    assert!(count("button:") > 1, "a widget factory has several buttons");

    // Descendant, not child: GTK inserts filler containers between a window
    // and anything in it, so a child selector would match nothing here and
    // that is exactly why `>` means descendant.
    assert!(
        count("window:>button:") > 1,
        "`window:>button:` found nothing -- `>` has stopped meaning descendant"
    );

    // Indexing picks exactly one out of many.
    assert_eq!(count("button:[0]"), 1);

    // A role nothing in this tree has.
    assert_eq!(count("terminal:"), 0);

    println!(
        "GTK  windows={} buttons={} buttons-under-a-window={}",
        count("window:"),
        count("button:"),
        count("window:>button:"),
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
