//! The semantic index -- the portable core.
//!
//! Everything above this crate (the MCP server) and everything below it (an
//! accessibility bridge, a compositor) is replaceable. This is not, so it is
//! the crate that must not acquire opinions about transport. It imports
//! `perspicax-node` and nothing else.
//!
//! Two traits describe the replaceable halves:
//!
//! - [`Ingest`] -- where nodes come from. AT-SPI2 over D-Bus today; a private
//!   Wayland protocol carrying AccessKit nodes later. The index does not care.
//! - [`HostView`] -- what only a compositor knows: is this rect visible, who
//!   owns this surface, dispatch this input. Smithay today; a GNOME extension
//!   or a KWin plugin later. The index does not care about that either.
//!
//! The refusal gate ([`Refusal`], [`check_actable`]) lives here rather than in
//! the MCP server, and that placement is deliberate: a future host that talks
//! to agents some other way must not be able to route around it.
//!
//! Between those two traits sit the three things that make a tree of nodes
//! addressable:
//!
//! - [`Interner`] -- an ingest path's own keys in, opaque [`NodeId`]s out, and
//!   never the same id twice.
//! - [`Selector`] -- the addressing scheme a human or a model can write down,
//!   as opposed to one minted at runtime.
//! - [`Index`] -- the cache, the tree order both of the above depend on, and
//!   the [`Delta`] computation that keeps a subscription from being a poll.
//!
//! And beneath [`HostView`], one more thing that could have lived in a
//! compositor and deliberately does not: [`HostFacts`] and [`judge`], the plain
//! data a host publishes and the arithmetic that turns it into a
//! [`Visibility`]. Occlusion decided here is occlusion decided once, testable
//! without Wayland, and identical across every host -- see [`mod@host`].

pub mod cache;
pub mod host;
pub mod id;
pub mod join;
pub mod receipt;
pub mod selector;

use core::future::Future;

use perspicax_node::{Node, NodeId, ObservedNode, Origin, Rect, SurfaceId, Visibility};

pub use crate::{
    cache::{Delta, Index},
    host::{HostFacts, Judgement, SurfaceFacts, Tally, judge},
    id::Interner,
    join::{Evidence, Finding, Join, SurfaceClaim, WindowClaim, join},
    receipt::{DamageWitness, Receipt, Verb, WindowReceipt, WindowVerb, WindowWitness},
    selector::{Selector, SelectorParseError},
};

/// Why an agent was not allowed to act.
///
/// Refusals are a feature and not an error path. Each variant carries what the
/// agent would need to recover on its own -- the occluding surface so it can
/// raise it, the match count so it can narrow a selector -- because a refusal
/// that only says "no" converts a recoverable situation into a retry loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The node is real but covered. Raise `by` and try again.
    Occluded { by: SurfaceId },
    /// Inside its window but scrolled out of view. Scroll it in and try again.
    Clipped,
    /// Its surface is not mapped -- a hidden window, an unopened menu.
    Unmapped,
    /// Not wholly on any monitor. Move its window onto one and try again.
    OffScreen,
    /// On a workspace that is not showing, numbered from 1.
    OtherWorkspace { workspace: u16 },
    /// A tab behind another in its window's tab group: `shown` is in front.
    InactiveTab { shown: SurfaceId },
    /// No compositor has judged this node's visibility. Fails closed.
    Unjudged,
    /// No compositor has attributed this node to a process, so no capability
    /// decision can be made about it. Fails closed.
    Unattributed,
    /// The node's surface has changed since this node was read, and the index
    /// has not caught up. Acting on it would be acting on the past.
    Stale { frames: u32 },
    /// The agent holds no capability for this node's origin.
    NoCapability { origin: Box<Origin> },
    /// The selector matched more than one node. Carries the count so the agent
    /// can decide whether to narrow it or enumerate.
    AmbiguousSelector { matches: usize },
    /// The selector matched nothing.
    NotFound,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Occluded { by } => write!(f, "node occluded by surface {}", by.0),
            Self::Clipped => write!(f, "node is scrolled out of its own viewport"),
            Self::Unmapped => write!(f, "node's surface is not mapped"),
            Self::OffScreen => write!(f, "node is not wholly on any output"),
            Self::InactiveTab { shown } => write!(
                f,
                "node's window is a tab behind surface {}, the one showing in its group",
                shown.0
            ),
            Self::OtherWorkspace { workspace } => {
                write!(
                    f,
                    "node's window is on workspace {workspace}, which is not showing"
                )
            }
            Self::Unjudged => write!(f, "node visibility has not been judged by a compositor"),
            Self::Unattributed => write!(f, "node has no attributed origin"),
            Self::Stale { frames: 0 } => {
                write!(f, "node's subtree was invalidated and has not been re-read")
            }
            Self::Stale { frames } => write!(f, "node is stale by {frames} frame(s) of damage"),
            Self::NoCapability { origin } => write!(f, "no capability for origin {origin:?}"),
            Self::AmbiguousSelector { matches } => write!(f, "selector matched {matches} nodes"),
            Self::NotFound => write!(f, "selector matched no nodes"),
        }
    }
}

impl core::error::Error for Refusal {}

/// Whose applications an agent may act on. Published by the host, consulted
/// by [`Index::actable`] after [`check_actable`].
///
/// This is the capability layer, and it is narrow on purpose. It does not ask
/// whether a *node* is safe. The visibility gate already does that. It asks
/// whether the *person* at the seat has agreed to an agent driving this
/// application at all. On a headless compositor there is no person, and every
/// client is on a private socket an agent set up, so the answer is
/// [`Consent::Everyone`]. On a seat the person launched most of what is on
/// screen, and the default is only what perspicax spawned itself.
///
/// The default is [`Consent::Nobody`], so a host that forgets to publish
/// consent produces no actable node. This is the same fail-closed rule the
/// visibility gate follows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Consent {
    /// No application may be acted on.
    #[default]
    Nobody,
    /// Only processes with these pids: the ones the host started for an
    /// agent. A process they start in turn is *not* included. Following
    /// ancestry would mean a terminal perspicax spawned could extend consent
    /// to anything typed into it.
    Spawned(Vec<u32>),
    /// Every attributed application.
    Everyone,
}

impl Consent {
    /// Whether an agent may act on something drawn by `origin`. Never for an
    /// unattributed origin, whatever the policy: a policy about processes
    /// cannot say yes to a process nobody has identified.
    #[must_use]
    ///
    /// An X11 origin is judged by its server, never by its client: every X
    /// client shares one Xwayland, and X11 lets any of them read and inject
    /// into the others, so consenting to one X client is consenting to all of
    /// them. The Xwayland perspicax starts for a person is never among the
    /// processes it spawned for an agent, so on a seat X11 applications are
    /// refused unless consent is everyone.
    pub fn permits(&self, origin: &Origin) -> bool {
        let pid = match origin {
            Origin::Unattributed => return false,
            Origin::Process(process) => process.pid,
            Origin::X11(x11) => x11.server,
        };
        match self {
            Self::Nobody => false,
            Self::Spawned(pids) => pids.contains(&pid),
            Self::Everyone => true,
        }
    }
}

/// The gate. Every act path goes through here.
///
/// Both conditions fail closed, and both defaults are un-actable, so an ingest
/// or host implementation that simply forgets to fill a field cannot produce a
/// clickable node by omission.
///
/// # Errors
///
/// Returns the [`Refusal`] describing why the node may not be acted on.
pub fn check_actable(node: &ObservedNode) -> Result<(), Refusal> {
    match &node.origin {
        Origin::Unattributed => return Err(Refusal::Unattributed),
        Origin::Process(_) | Origin::X11(_) => {}
    }
    match &node.visibility {
        Visibility::Visible => Ok(()),
        Visibility::Occluded { by } => Err(Refusal::Occluded { by: *by }),
        Visibility::Clipped => Err(Refusal::Clipped),
        Visibility::Unmapped => Err(Refusal::Unmapped),
        Visibility::OffScreen => Err(Refusal::OffScreen),
        Visibility::OtherWorkspace { workspace } => Err(Refusal::OtherWorkspace {
            workspace: *workspace,
        }),
        Visibility::InactiveTab { shown } => Err(Refusal::InactiveTab { shown: *shown }),
        Visibility::Unknown => Err(Refusal::Unjudged),
    }
}

/// The gate for acting on a whole window. Every window verb goes through
/// here, as every node verb goes through [`check_actable`].
///
/// A window is addressed by surface, so there is no node to judge visible:
/// what is checked instead is that the window exists, that it is attributed,
/// and that the agent holds consent for whoever drew it. Bringing a tab
/// forward hides the tab in front, so consent for that one is needed too.
/// And it is only done where the person can already see the group: a group
/// on a hidden workspace, or minimized, is refused rather than brought into
/// view, because an agent never switches what the person is looking at.
///
/// # Errors
///
/// The [`Refusal`] saying why not.
pub fn check_window(
    facts: &HostFacts,
    surface: SurfaceId,
    verb: WindowVerb,
) -> Result<&SurfaceFacts, Refusal> {
    let window = facts.surface(surface).ok_or(Refusal::NotFound)?;
    let consented = |window: &SurfaceFacts| match &window.origin {
        Origin::Unattributed => Err(Refusal::Unattributed),
        origin if facts.consent().permits(origin) => Ok(()),
        origin => Err(Refusal::NoCapability {
            origin: Box::new(origin.clone()),
        }),
    };
    consented(window)?;
    if verb == WindowVerb::Forward
        && let Some(front) = window.behind_tab
    {
        let front = facts.surface(front).ok_or(Refusal::NotFound)?;
        consented(front)?;
        if let Some(workspace) = front.off_workspace {
            return Err(Refusal::OtherWorkspace { workspace });
        }
        if !front.mapped {
            return Err(Refusal::Unmapped);
        }
    }
    Ok(window)
}

/// Where nodes come from.
///
/// Implementations are expected to be *push*-shaped wherever the underlying
/// source allows it. Polling a whole tree is the failure mode this project was
/// started to fix, not a fallback to reach for.
///
/// # Why this trait is async, and why that costs this crate nothing
///
/// Every real source of nodes is asynchronous. AT-SPI2 is D-Bus, which the
/// `atspi` crate exposes over zbus; the eventual fast path is a Wayland
/// protocol. Making implementations hide that behind a blocking facade would
/// mean burying a runtime inside a library and, worse, would put a *polling*
/// shape on `drain_changes` -- the exact thing the paragraph above rejects.
///
/// Being async does not compromise this crate's portability, because
/// [`Future`] lives in `core`: there is no runtime dependency here, and there
/// is not going to be one. The runtime belongs to whoever drives the trait.
///
/// # Why `impl Future + Send` rather than `async fn`
///
/// Written as a bare `async fn`, the returned future carries no auto-trait
/// bounds, so a caller that wants to `spawn` the driver onto a multi-threaded
/// runtime cannot ask for `Send` and gets a famously indirect error instead.
/// Spelling the bound here makes it an obligation of the implementation, which
/// is where it can actually be satisfied -- zbus proxies are `Send`, so this
/// costs the AT-SPI path nothing, and it fails loudly at the definition rather
/// than quietly at the first `tokio::spawn`.
pub trait Ingest {
    /// The implementation's own error type -- D-Bus failures for an AT-SPI
    /// bridge, protocol errors for a Wayland one.
    type Error;

    /// Read a subtree in as few round trips as the source permits.
    ///
    /// "As few as the source permits" is doing real work in that sentence, and
    /// what a source permits is not what it advertises. A warm GTK application
    /// answers in one `Cache.GetItems` call; the Qt widget gallery offers the
    /// same interface and answers it with an empty array; a cold GTK one
    /// answers with a fraction of its tree. All three must be walked node by
    /// node to get an answer, and only the first can avoid it. Every one of
    /// those is a legitimate implementation of this method, and the distance
    /// between them is the measurement the project turns on.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying source cannot be read.
    fn snapshot(
        &mut self,
        root: NodeId,
    ) -> impl Future<Output = Result<Vec<ObservedNode>, Self::Error>> + Send;

    /// Drain whatever the source has volunteered since the last call.
    ///
    /// Volunteered, not fetched. An implementation that answers this by going
    /// and looking has misread the trait.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the underlying source cannot be read.
    fn drain_changes(&mut self) -> impl Future<Output = Result<Vec<Change>, Self::Error>> + Send;
}

/// A change the ingest path volunteered.
///
/// `PartialEq` because a change is a value, and two of them being equal is a
/// question worth asking -- an ingest path that reports the same change twice
/// has turned a delta stream back into a poll, and a test can only say so if
/// changes compare.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A node appeared or its contents changed.
    Upserted { id: NodeId, node: Box<Node> },
    /// A node went away.
    Removed { id: NodeId },
    /// A subtree changed shape and must be re-read.
    SubtreeInvalidated { root: NodeId },
}

/// What only a compositor knows.
///
/// The three questions here are the entire reason the semantic index lives
/// inside a compositor rather than beside one. Nothing else in this crate is
/// unusual; this trait is the argument.
pub trait HostView {
    /// The implementation's own error type.
    type Error;

    /// Who owns this surface. From the client's credentials, never its claims.
    fn origin(&self, surface: SurfaceId) -> Origin;

    /// Whether a window-relative rect on this surface can actually be seen,
    /// accounting for z-order, the opaque regions of everything above it, and
    /// whether the surface is mapped at all.
    fn visibility(&self, surface: SurfaceId, rect: Rect) -> Visibility;

    /// How many frames of damage this surface has taken, ever. Monotonic.
    ///
    /// Deliberately a counter and not a verdict. This was written as
    /// `staleness` before M2 measured what damage is, and a host cannot answer
    /// "is this stale" -- staleness is a comparison against what *the reader*
    /// last reconciled, and the host does not know that number. It is also not
    /// a whole-surface question: an idle GTK application repaints its window
    /// about forty times a second, so a host that answered "stale" whenever
    /// damage had arrived would refuse every node in it permanently.
    /// [`SurfaceFacts::damage_since`] is where the counter and the reconcile
    /// point are compared, and [`Index::reconcile`] is what moves the latter.
    ///
    /// [`SurfaceFacts::damage_since`]: crate::SurfaceFacts::damage_since
    /// [`Index::reconcile`]: crate::Index::reconcile
    fn damage_generation(&self, surface: SurfaceId) -> u64;

    /// Dispatch input through the compositor's own input path, so focus,
    /// grabs and z-order stay correct by construction.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the input could not be dispatched.
    fn dispatch(&mut self, surface: SurfaceId, action: &Action) -> Result<(), Self::Error>;
}

/// An input action to be dispatched at a node.
///
/// Deliberately not AccessKit's `Action`: that describes what a *widget*
/// supports ("this is a button, it can be invoked"), whereas this describes
/// what a *seat* does ("move the pointer here and press button 1"). Going
/// through real input rather than an accessibility bridge's `DoAction` is what
/// keeps focus and grabs honest, so the two vocabularies stay separate.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Move the pointer to a point and click it.
    Click { at: Rect, button: PointerButton },
    /// Type text through the seat's keyboard, xkb-mapped.
    Type { text: String },
    /// Scroll at a point.
    Scroll { at: Rect, dx: f64, dy: f64 },
    /// Give this surface keyboard focus.
    Focus,
    /// Ask this surface's window to close.
    Close,
    /// Bring this surface's window to the front of its tab group.
    Forward,
}

/// A pointer button, in the usual left/middle/right sense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Middle,
    Right,
}

#[cfg(test)]
mod tests {
    use core::{
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use super::*;
    use perspicax_node::{ProcessOrigin, Role};

    fn attributed() -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid: 9182,
            exe: Some("/usr/bin/gedit".into()),
            cgroup: None,
            sandbox: None,
        }))
    }

    fn node_with(origin: Origin, visibility: Visibility) -> ObservedNode {
        let mut n = ObservedNode::unjoined(NodeId(1), Node::new(Role::Button));
        n.origin = origin;
        n.visibility = visibility;
        n
    }

    #[test]
    fn a_visible_attributed_node_is_actable() {
        assert!(check_actable(&node_with(attributed(), Visibility::Visible)).is_ok());
    }

    #[test]
    fn an_occluded_node_names_what_covers_it() {
        let err = check_actable(&node_with(
            attributed(),
            Visibility::Occluded { by: SurfaceId(7) },
        ))
        .unwrap_err();
        assert_eq!(err, Refusal::Occluded { by: SurfaceId(7) });
        assert_eq!(err.to_string(), "node occluded by surface 7");
    }

    /// The two fail-closed cases, which are the ones that matter. A node
    /// straight off an ingest path has neither field set, and must be refused
    /// on both counts rather than sliding through on a default.
    #[test]
    fn an_unjoined_node_fails_closed() {
        let raw = ObservedNode::unjoined(NodeId(1), Node::new(Role::Button));
        assert_eq!(check_actable(&raw).unwrap_err(), Refusal::Unattributed);
    }

    #[test]
    fn attribution_alone_is_not_enough() {
        let judged_by_nobody = node_with(attributed(), Visibility::Unknown);
        assert_eq!(
            check_actable(&judged_by_nobody).unwrap_err(),
            Refusal::Unjudged
        );
    }

    /// A minimal executor, so this crate can exercise its own async seam
    /// without acquiring a runtime -- not even as a dev-dependency. The mock
    /// below never suspends, so one poll always completes it; reaching
    /// `Pending` would mean it had grown a suspension point it is not supposed
    /// to have, which is worth a panic rather than a spin.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => unreachable!("MockIngest must not suspend"),
        }
    }

    /// The smallest thing that can satisfy [`Ingest`]. It exists to hold the
    /// trait's shape still: that it is implementable with a plain `async fn`,
    /// and that what comes back is `Send`.
    struct MockIngest {
        nodes: Vec<ObservedNode>,
        changes: Vec<Change>,
    }

    impl Ingest for MockIngest {
        type Error = core::convert::Infallible;

        async fn snapshot(&mut self, _root: NodeId) -> Result<Vec<ObservedNode>, Self::Error> {
            Ok(self.nodes.clone())
        }

        async fn drain_changes(&mut self) -> Result<Vec<Change>, Self::Error> {
            Ok(core::mem::take(&mut self.changes))
        }
    }

    #[test]
    fn the_async_seam_round_trips() {
        let mut ingest = MockIngest {
            nodes: vec![ObservedNode::unjoined(NodeId(1), Node::new(Role::Button))],
            changes: vec![Change::Removed { id: NodeId(1) }],
        };

        assert_eq!(block_on(ingest.snapshot(NodeId(0))).unwrap().len(), 1);
        assert_eq!(block_on(ingest.drain_changes()).unwrap().len(), 1);

        // Drained means drained. A source that re-reports what it has already
        // volunteered turns a delta stream back into a poll.
        assert!(block_on(ingest.drain_changes()).unwrap().is_empty());
    }

    /// A compile-time assertion wearing a test's clothes. If [`Ingest`]'s
    /// futures ever stop being `Send`, this stops building -- which is the
    /// whole reason the bound is spelled on the trait rather than left to
    /// inference at each call site.
    #[test]
    fn ingest_futures_are_send() {
        fn assert_send<T: Send>(_: T) {}

        let mut ingest = MockIngest {
            nodes: Vec::new(),
            changes: Vec::new(),
        };
        assert_send(ingest.snapshot(NodeId(0)));
        assert_send(ingest.drain_changes());
    }

    #[test]
    fn no_consent_policy_permits_an_unattributed_origin() {
        for consent in [
            Consent::Nobody,
            Consent::Spawned(vec![0]),
            Consent::Everyone,
        ] {
            assert!(!consent.permits(&Origin::Unattributed), "{consent:?}");
        }
    }

    #[test]
    fn an_x11_origin_is_consented_by_its_server_not_its_client() {
        let x11 = Origin::X11(Box::new(perspicax_node::X11Origin {
            server: 700,
            client: Some(ProcessOrigin {
                pid: 900,
                exe: None,
                cgroup: None,
                sandbox: None,
            }),
            basis: perspicax_node::X11Basis::XRes,
        }));
        assert!(
            !Consent::Spawned(vec![900]).permits(&x11),
            "the client is not the unit"
        );
        assert!(Consent::Spawned(vec![700]).permits(&x11));
        assert!(Consent::Everyone.permits(&x11));
    }

    fn owned_by(pid: u32) -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid,
            exe: None,
            cgroup: None,
            sandbox: None,
        }))
    }

    /// Two tabs of one group: `1` behind `2`, both drawn by pid 10.
    fn tabs(front: SurfaceFacts, consent: Consent) -> HostFacts {
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), area)
                    .owned_by(owned_by(10))
                    .behind_tab(SurfaceId(2)),
                front.owned_by(owned_by(10)),
            ],
            1,
        )
        .with_consent(consent)
    }

    fn front() -> SurfaceFacts {
        SurfaceFacts::new(SurfaceId(2), Rect::new(0.0, 0.0, 400.0, 300.0))
    }

    #[test]
    fn a_window_verb_needs_a_window_an_origin_and_consent() {
        let facts = tabs(front(), Consent::Everyone);
        assert_eq!(
            check_window(&facts, SurfaceId(9), WindowVerb::Close).unwrap_err(),
            Refusal::NotFound
        );
        assert!(check_window(&facts, SurfaceId(2), WindowVerb::Close).is_ok());

        let facts = tabs(front(), Consent::Spawned(vec![11]));
        assert!(matches!(
            check_window(&facts, SurfaceId(2), WindowVerb::Close),
            Err(Refusal::NoCapability { .. })
        ));

        let unattributed = HostFacts::bottom_to_top(
            [SurfaceFacts::new(
                SurfaceId(1),
                Rect::new(0.0, 0.0, 1.0, 1.0),
            )],
            1,
        )
        .with_consent(Consent::Everyone);
        assert_eq!(
            check_window(&unattributed, SurfaceId(1), WindowVerb::Close).unwrap_err(),
            Refusal::Unattributed
        );
    }

    #[test]
    fn a_tab_comes_forward_only_where_the_person_can_already_see_its_group() {
        let showing = tabs(front(), Consent::Everyone);
        assert!(check_window(&showing, SurfaceId(1), WindowVerb::Forward).is_ok());
        assert!(
            check_window(&showing, SurfaceId(2), WindowVerb::Forward).is_ok(),
            "already in front: nothing to refuse"
        );

        let elsewhere = tabs(front().on_workspace(3), Consent::Everyone);
        assert_eq!(
            check_window(&elsewhere, SurfaceId(1), WindowVerb::Forward).unwrap_err(),
            Refusal::OtherWorkspace { workspace: 3 }
        );
        assert!(
            check_window(&elsewhere, SurfaceId(1), WindowVerb::Close).is_ok(),
            "closing does not show anything"
        );

        let minimized = tabs(front().unmapped(), Consent::Everyone);
        assert_eq!(
            check_window(&minimized, SurfaceId(1), WindowVerb::Forward).unwrap_err(),
            Refusal::Unmapped
        );
    }

    #[test]
    fn bringing_a_tab_forward_needs_consent_for_the_one_it_hides() {
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), area)
                    .owned_by(owned_by(10))
                    .behind_tab(SurfaceId(2)),
                SurfaceFacts::new(SurfaceId(2), area).owned_by(owned_by(20)),
            ],
            1,
        )
        .with_consent(Consent::Spawned(vec![10]));
        assert!(matches!(
            check_window(&facts, SurfaceId(1), WindowVerb::Forward),
            Err(Refusal::NoCapability { .. })
        ));
    }
}
