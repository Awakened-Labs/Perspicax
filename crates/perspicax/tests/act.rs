//! The v1 demo, asserted rather than described.
//!
//! M3's exit criterion, in one sentence: *an agent observes a window, resolves
//! a selector, clicks it, gets a receipt — and gets a refusal naming the
//! occluding surface, against a GTK app and a Qt app, with zero screenshots
//! taken.* The two tests below are that sentence's two halves.
//!
//! # Why the receipt half runs each toolkit alone
//!
//! Windows cascade by 32 px, so with both toolkits hosted at once one covers
//! nearly all of the other -- M2 measured Qt's gallery going from 124 visible
//! nodes to 2 when the GTK window was placed over it. That is exactly what the
//! refusal half needs and exactly what makes "click a control in each toolkit"
//! unreliable: the lower application may have no actable labelled control left.
//!
//! So the receipt half hosts one toolkit per run, which is also where this
//! project's oldest open question about clicking gets its answer -- GTK and Qt
//! need not agree about what a click does, and a run per toolkit is the only
//! arrangement in which each one's answer is its own.
//!
//! # Why the selector is computed rather than written down
//!
//! A selector hard-coded against `gtk4-widget-factory` would be a test of that
//! application's current layout. The tests instead pick a control by *role and
//! actability* and then build the selector that names it -- `Button:Cancel[2]`
//! -- in the grammar an agent would have used. That exercises the grammar,
//! including its index form, and survives a toolkit release.
//!
//! `#[ignore]`d because it needs a live accessibility bus and two installed
//! sample applications. `ci/live-tests.sh` runs it with `--include-ignored`.

use std::{path::PathBuf, process::Command, sync::Arc, sync::mpsc, thread, time::Duration};

use perspicax::{
    desk::Desk,
    observe::{self, App},
    session,
};
use perspicax_compositor::{Backend, Config, Facts, Host, Requests, Stop};
use perspicax_index::{Index, PointerButton, Receipt, Refusal, Selector, Verb};
use perspicax_mcp::{Denied, Desktop, Perspicax, dto};
use perspicax_node::{NodeId, Origin, Role, SurfaceId};

/// How long the applications get to map their windows and populate their
/// accessibility trees before anything is read.
///
/// A toolkit maps a window and *then* fills in its tree, so reading too early
/// finds a window with nothing in it. Generous rather than tuned: the failure
/// this guards against is a flaky test that looks like a broken join.
const SETTLE: Duration = Duration::from_secs(8);

/// A click, and everything worth knowing about it afterwards.
struct Act {
    /// The application it was aimed at, by bus name.
    app: String,
    /// The selector, as an agent would have written it.
    selector: String,
    /// The node as an agent saw it **before** the act, which is what makes the
    /// gate's answer checkable rather than merely reported.
    seen: dto::Node,
    /// What came back.
    outcome: Result<Receipt, Denied>,
}

/// What one run of the compositor found and did.
struct Drive {
    /// Every application this compositor hosted and read, by bus name.
    apps: Vec<String>,
    /// `window_list`, exactly as the tool answered it.
    windows: serde_json::Value,
    /// `screenshot`, and whether it called itself an error.
    screenshot: serde_json::Value,
    screenshot_failed: Option<bool>,
    /// One act per application that offered an actable control.
    acts: Vec<Act>,
    /// The covered control, and what the gate said about acting on it.
    covered: Option<Act>,
    /// Nodes that no compositor attributed to a process, which should be
    /// none of them -- see [`drive`] for the one exception.
    unattributed: Vec<u64>,
    /// How many nodes were read in total.
    total: usize,
}

#[test]
#[ignore = "needs a live accessibility bus, gtk-4-examples and qt6-base-examples"]
fn an_agent_clicks_a_control_and_gets_a_receipt_in_each_toolkit() {
    for spawn in [
        vec!["gtk4-widget-factory".to_owned()],
        vec![
            qt_gallery()
                .expect("qt6-base-examples must be installed")
                .to_string_lossy()
                .into_owned(),
        ],
    ] {
        let toolkit = spawn[0].clone();
        let drive = run(vec![spawn]);

        assert_eq!(
            drive.apps.len(),
            1,
            "{toolkit}: expected exactly the application this compositor spawned, got {:?}",
            drive.apps
        );
        assert!(
            drive.unattributed.is_empty(),
            "{toolkit}: {} of {} nodes were never attributed to a process: {:?}",
            drive.unattributed.len(),
            drive.total,
            drive.unattributed
        );

        let act = drive
            .acts
            .first()
            .unwrap_or_else(|| panic!("{toolkit}: no actable labelled control was found at all"));

        // The gate said yes before the act, and the receipt has to agree with
        // it about which node was meant.
        assert!(
            act.seen.actable,
            "{toolkit}: {} was not actable",
            act.selector
        );
        let receipt = act
            .outcome
            .as_ref()
            .unwrap_or_else(|e| panic!("{toolkit}: {} was not dispatched: {e}", act.selector));

        assert_eq!(receipt.selector, act.selector);
        assert_eq!(receipt.node.0, act.seen.node);
        assert_eq!(Some(receipt.surface.0), act.seen.surface);
        assert!(
            matches!(receipt.origin, Origin::Process(_)),
            "{toolkit}: a receipt for an unattributed node should have been impossible: {:?}",
            receipt.origin
        );
        assert_eq!(receipt.verb, Verb::Click(PointerButton::Left));
        assert_eq!(receipt.damage_window, perspicax::act::DAMAGE_WINDOW);

        // The two durations are separate fields for a reason, and this is the
        // reason stated as a relation rather than as a magic number: dispatch
        // is a round trip to the compositor's thread, and the damage window is
        // deliberate patience. A constant here would be a timing assertion on
        // a shared runner, which is a flake waiting to happen; the relation is
        // the property worth holding and cannot flake without something being
        // genuinely wrong.
        assert!(
            receipt.dispatch < receipt.damage_window,
            "{toolkit}: dispatch took {:?}, which is not the cheap half of a {:?} window",
            receipt.dispatch,
            receipt.damage_window
        );

        // What the pixels did is EVIDENCE and not a verdict, so what is
        // asserted is that evidence was gathered -- not what it says. An idle
        // GTK window repaints about forty times a second and an idle Qt one
        // about once every two, so a test demanding `OnTarget` would pass on
        // one toolkit and be flaky on the other for reasons having nothing to
        // do with whether the click worked. The number is reported instead.
        println!(
            "{toolkit} (on the bus as {}): clicked {} (node {}) -- dispatch {:?}, \
             damage {:?}, focus {:?} -> {:?}",
            act.app,
            receipt.selector,
            receipt.node.0,
            receipt.dispatch,
            receipt.damage,
            receipt.focus_before.map(|s| s.0),
            receipt.focus_after.map(|s| s.0),
        );

        // Zero screenshots taken, and structurally so: the only pixel path
        // there is refuses, and the demo above did not need it.
        assert_eq!(drive.screenshot_failed, Some(true), "{toolkit}");
        assert_eq!(drive.screenshot["captured"], false, "{toolkit}");
        assert_eq!(drive.screenshot["reason"], "no_renderer", "{toolkit}");
    }
}

#[test]
#[ignore = "needs a live accessibility bus, gtk-4-examples and qt6-base-examples"]
fn a_covered_control_is_refused_and_the_refusal_names_the_surface_in_the_way() {
    let gallery = qt_gallery().expect("qt6-base-examples must be installed");
    let drive = run(vec![
        vec![gallery.to_string_lossy().into_owned()],
        vec!["gtk4-widget-factory".to_owned()],
    ]);

    assert_eq!(
        drive.apps.len(),
        2,
        "expected both toolkits to be hosted and read, got {:?}",
        drive.apps
    );

    // Both windows are on the wire, each carrying the credentials of the
    // process that drew it -- which is the half of this claim that no library
    // outside a compositor can make at all.
    assert_eq!(drive.windows["count"], 2, "{}", drive.windows);
    for window in drive.windows["items"]
        .as_array()
        .expect("window_list answers with a list")
    {
        assert!(
            window["rendered_by"]["pid"].is_u64(),
            "a hosted window with no attributed process: {window}"
        );
        assert!(window["nodes"].as_u64().is_some_and(|nodes| nodes > 0));
    }

    let covered = drive
        .covered
        .as_ref()
        .expect("with two overlapping windows, something must be covered");

    // The gate refused, and named what to raise. This is the sentence the
    // milestone exists to be able to say.
    let by = match &covered.outcome {
        Err(Denied::Refused(Refusal::Occluded { by })) => *by,
        other => panic!(
            "{} should have been refused as occluded, got {other:?}",
            covered.selector
        ),
    };

    // The surface named is the other application's, not something invented:
    // the agent-facing projection and the refusal have to agree about it.
    let occluding: Vec<u64> = drive.windows["items"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|window| window["surface"].as_u64())
        .filter(|surface| Some(*surface) != covered.seen.surface)
        .collect();
    assert_eq!(
        occluding,
        vec![by.0],
        "the refusal names surface {}, which is not the other window's",
        by.0
    );
    assert_eq!(
        covered.seen.refused.as_ref().map(|r| r.kind),
        Some("occluded"),
        "`observe` and `act` disagreed about the same node"
    );
    assert_eq!(
        covered.seen.refused.as_ref().and_then(|r| r.occluded_by),
        Some(by.0)
    );

    println!(
        "{}: refused {} (node {}) on surface {:?} -- occluded by surface {}",
        covered.app, covered.selector, covered.seen.node, covered.seen.surface, by.0
    );

    assert_eq!(drive.screenshot_failed, Some(true));
    assert_eq!(drive.screenshot["captured"], false);
}

/// Host the given applications, read them, and drive the agent interface
/// against what was found.
///
/// The shape is `demo.rs`'s: `perspicax_compositor::run` blocks the main test
/// thread because Wayland state is not `Send` and the loop owns it, so the work
/// happens on a worker that asks the compositor to stop when it is done. A
/// deadline is a backstop only -- reading a Qt tree takes seconds nobody can
/// predict.
fn run(spawn: Vec<Vec<String>>) -> Drive {
    let _registry = session::Registry::ensure().expect("an accessibility registry");

    let facts = Facts::new();
    let stop = Stop::new();
    // One `Requests`, shared: it hands its receiving end to exactly one loop,
    // so a `Host` built from a second channel would be one nobody hears.
    let requests = Requests::new();
    let config = Config {
        backend: Backend::headless((1280, 800)),
        spawn,
        env: session::accessibility_env(),
        run_for: Some(Duration::from_secs(180)),
        config: None,
        socket: None,
        xwayland: false,
    };

    let (sender, receiver) = mpsc::channel();
    let worker = {
        let (facts, stop) = (facts.clone(), stop.clone());
        let host = Host::new(&facts, &requests);
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            let drive = runtime.block_on(drive(&facts, &host));
            let _ = sender.send(drive);
            stop.request();
        })
    };

    perspicax_compositor::run(&config, &facts, &requests, &stop).expect("the compositor runs");
    worker.join().expect("the worker finishes");
    receiver.recv().expect("the worker reports")
}

/// Read the desktop, then act on it the way an agent would.
async fn drive(facts: &Facts, host: &Host) -> Drive {
    tokio::time::sleep(SETTLE).await;
    let _ = perspicax_atspi::enable().await;
    let reading = observe::observe(facts)
        .await
        .expect("the accessibility bus is readable");

    let index = &reading.index;
    let snapshot = facts.read();
    let total = index.len();

    // Every node except each application's own root, and the exception is a
    // property of the schema rather than a shortfall in the join: an AT-SPI
    // root is the bus's handle for a *process*, sitting above the windows and
    // drawn on no surface at all. `join_subtree` attributes a window and
    // everything beneath it, which is every node that was actually rendered.
    let roots: Vec<NodeId> = reading.apps.iter().map(|app| app.root).collect();
    let unattributed: Vec<u64> = index
        .preorder()
        .into_iter()
        .filter(|id| !roots.contains(id))
        .filter(|id| {
            index
                .get(*id)
                .is_some_and(|node| matches!(node.origin, Origin::Unattributed))
        })
        .map(|id| id.0)
        .collect();

    // Every target is chosen against ONE description of the screen, before
    // anything is dispatched. Choosing them as we go would mean the second
    // choice was made against a screen the first one had already changed.
    let targets: Vec<(String, String, dto::Node)> = reading
        .apps
        .iter()
        .filter_map(|app| {
            let id = clickable(index, app)?;
            Some((
                app.name.clone(),
                selector_for(index, id)?,
                dto::Node::of(index, id)?,
            ))
        })
        .collect();
    let target_covered: Option<(String, String, dto::Node)> = reading.apps.iter().find_map(|app| {
        let (id, _) = covered(index, app)?;
        Some((
            app.name.clone(),
            selector_for(index, id)?,
            dto::Node::of(index, id)?,
        ))
    });

    let apps: Vec<String> = reading.apps.iter().map(|app| app.name.clone()).collect();
    let desk = Arc::new(Desk::new(facts, host));
    desk.publish(reading.index);

    let click = |(app, selector, seen): (String, String, dto::Node)| Act {
        outcome: desk.act(
            &Selector::parse(&selector).expect("built from the grammar"),
            &Verb::Click(PointerButton::Left),
        ),
        app,
        selector,
        seen,
    };

    // The refusal first, deliberately: it must happen without anything having
    // been dispatched, and doing it before any successful act is how that stays
    // true rather than merely likely.
    let covered = target_covered.map(click);
    let acts = targets.into_iter().map(click).collect();

    // The two tools that take no arguments, called for real rather than
    // projected -- `window_list` is the agent's entry point and `screenshot` is
    // the one that has to refuse.
    let shared: Arc<dyn Desktop> = Arc::<Desk>::clone(&desk);
    let server = Perspicax::new(shared);
    let windows = server.window_list().await.expect("window_list answers");
    let shot = server.screenshot().await.expect("screenshot answers");

    // A window with nothing on it would make the assertions above vacuous, so
    // the count is reported either way.
    println!(
        "{total} nodes over {} surface(s); {} unattributed",
        snapshot.surfaces().len(),
        unattributed.len()
    );

    Drive {
        apps,
        windows: windows.structured_content.expect("structured"),
        screenshot: shot.structured_content.expect("structured"),
        screenshot_failed: shot.is_error,
        acts,
        covered,
        unattributed,
        total,
    }
}

/// A control worth clicking: labelled, actable, and not one that would take the
/// application down with it.
///
/// The exclusions are not squeamishness. `gtk4-widget-factory` and Qt's gallery
/// both carry window-decoration buttons in their accessible trees, and a test
/// that clicked "Close" would pass once and then have nothing left to read.
fn clickable(index: &Index, app: &App) -> Option<NodeId> {
    app.nodes(index).into_iter().find(|id| {
        index.get(*id).is_some_and(|node| {
            matches!(node.node.role(), Role::Button | Role::CheckBox)
                && index.actable(*id).is_ok()
                && node.node.label().is_some_and(|label| !destructive(label))
        })
    })
}

/// A control this application shows as covered, and what covers it.
///
/// Labelled, because the point is to name it in a selector: a refusal for a
/// node an agent could not have addressed would not be the claim being made.
///
/// A button or a checkbox for preference, and a window node only if there is
/// nothing better. Both are real nodes and both are refused identically, but
/// the sentence this demo exists to say is "an agent tried to click a *control*
/// it could not see", and a window is not a control.
fn covered(index: &Index, app: &App) -> Option<(NodeId, SurfaceId)> {
    let occluded: Vec<(NodeId, SurfaceId)> = app
        .nodes(index)
        .into_iter()
        .filter_map(|id| {
            index.get(id)?.node.label()?;
            match index.actable(id) {
                Err(Refusal::Occluded { by }) => Some((id, by)),
                _ => None,
            }
        })
        .collect();

    occluded
        .iter()
        .find(|(id, _)| {
            index
                .get(*id)
                .is_some_and(|node| matches!(node.node.role(), Role::Button | Role::CheckBox))
        })
        .or_else(|| occluded.first())
        .copied()
}

fn destructive(label: &str) -> bool {
    let label = label.to_lowercase();
    ["quit", "close", "exit", "delete", "remove"]
        .iter()
        .any(|word| label.contains(word))
}

/// The selector that names exactly this node, in the grammar an agent writes.
///
/// The index form is added only when it is needed, which is both tidier and a
/// better test: `Button:Cancel` and `Button:Cancel[2]` take different paths
/// through the parser and through `resolve_all`, and a real toolkit tree
/// produces both.
fn selector_for(index: &Index, id: NodeId) -> Option<String> {
    let node = index.get(id)?;
    let base = format!("{:?}:{}", node.node.role(), node.node.label()?);
    let matches = index.resolve_all(&Selector::parse(&base).ok()?);
    let nth = matches.iter().position(|found| *found == id)?;
    Some(if matches.len() == 1 {
        base
    } else {
        format!("{base}[{nth}]")
    })
}

/// Where Debian keeps Qt's widget gallery.
///
/// Asked of `dpkg` rather than hard-coded, because the path is
/// architecture-qualified and would be right on exactly one runner.
/// `PERSPICAX_QT_GALLERY` overrides it for anyone whose distribution puts it
/// elsewhere.
fn qt_gallery() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PERSPICAX_QT_GALLERY") {
        return Some(PathBuf::from(path));
    }
    let listing = Command::new("dpkg")
        .args(["-L", "qt6-base-examples"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .find(|line| line.ends_with("/widgets/gallery/bin/gallery"))
        .map(PathBuf::from)
}
