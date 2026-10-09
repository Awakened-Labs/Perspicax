//! What an agent sees, and how it is marked.
//!
//! # Why a projection rather than serde on the schema
//!
//! `perspicax-node` derives nothing and `accesskit`'s `serde` feature is off,
//! so the wire format is a decision rather than a default -- and three things
//! follow from making it here instead of there.
//!
//! The wire stays still when the schema moves. `accesskit` is upstream and
//! gains properties with every release; a client parsing whatever the schema
//! happened to hold that week would break on a dependency bump nobody thought
//! was agent-facing.
//!
//! The agent gets a projection rather than a dump. An AccessKit node carries
//! text decorations, live-region politeness and page-breaking hints; none of
//! that helps a model decide what to click, and every field of it costs
//! context.
//!
//! And this is the natural place to **mark text untrusted**, which is the
//! milestone's whole read-path security posture. See [`Text`].
//!
//! # Ids on the wire are plain integers
//!
//! The plan called for `Display` and `FromStr` on `NodeId` so an agent could
//! hand an id back. It cannot be written: [`perspicax_node::NodeId`] is a
//! re-export of `accesskit::NodeId`, so both traits are foreign to both crates
//! and the orphan rule forbids the impls. It is also not needed. `accesskit`
//! already ships `From<u64>` and `From<NodeId> for u64`, JSON has integers, and
//! an id that is a number on the wire cannot be handed back in the wrong
//! lexical form. Ids are minted from one, so the 2^53 ceiling a JSON reader
//! imposes is not a ceiling anything can reach.

use perspicax_index::{
    DamageWitness, Delta, Drawn, HostFacts, Index, Receipt, Refusal, Shot, SurfaceFacts,
    SurfaceKind, WindowReceipt, WindowWitness,
};
use perspicax_node::{
    NodeId, ObservedNode, Orientation, Origin, Rect, SurfaceId, Toggled, X11Basis,
};
use serde::Serialize;

/// Who drew something, from the client's connection credentials.
///
/// Never from the application's own claim about itself -- that is the point of
/// the type. A universal screen API makes every rendered pixel an instruction
/// channel, and knowing which process authored a string is the only thing that
/// lets a reader decide how much of it to believe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    /// From the Wayland client's credentials.
    pub pid: u32,
    /// Resolved from `/proc/<pid>/exe`, when it is still readable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// Resolved from `/proc/<pid>/cgroup`. What a policy usually wants when the
    /// process is one of many inside a container or a user service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<String>,
    /// Flatpak or Snap identity, when the process carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// Present when the drawing process is an X11 client under Xwayland.
    /// `pid` above is then Xwayland itself -- the only process the
    /// compositor holds credentials for -- and this says which X client drew
    /// it and how that was learned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x11: Option<X11Provenance>,
}

impl Provenance {
    /// The provenance of an attributed origin. `None` means no compositor has
    /// attributed it, which is a real state and not a missing field.
    #[must_use]
    pub fn of(origin: &Origin) -> Option<Self> {
        match origin {
            Origin::Unattributed => None,
            Origin::Process(process) => Some(Self {
                pid: process.pid,
                exe: process.exe.clone(),
                cgroup: process.cgroup.clone(),
                sandbox: process.sandbox.clone(),
                x11: None,
            }),
            Origin::X11(x11) => Some(Self {
                pid: x11.server,
                exe: None,
                cgroup: None,
                sandbox: None,
                x11: Some(X11Provenance {
                    client_pid: x11.client.as_ref().map(|client| client.pid),
                    client_exe: x11.client.as_ref().and_then(|client| client.exe.clone()),
                    basis: match x11.basis {
                        X11Basis::XRes => "xres",
                        X11Basis::ClaimedPid => "claimed_pid",
                        X11Basis::ServerOnly => "server_only",
                    },
                }),
            }),
        }
    }
}

/// The X client behind an Xwayland surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct X11Provenance {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_exe: Option<String>,
    /// `xres` (the X server's own answer, trusted because perspicax started
    /// it), `claimed_pid` (the client's `_NET_WM_PID`, a hint only), or
    /// `server_only`.
    pub basis: &'static str,
}

/// Strings an application rendered, carried under a key that says what they are.
///
/// This is the injection defence, and it is a read-path property rather than an
/// act-path gate. A model that knows a label was drawn by a particular process
/// can apply its own limits to it; one that reads it as ordinary tool output
/// cannot. The marking therefore lives in the **key** an agent has to type to
/// reach the string -- `untrusted_text.label`, never a bare `label` -- so it
/// cannot be skimmed past, and the provenance sits inside the same object so it
/// is present at the point of use rather than in a preamble the model has to
/// have remembered.
///
/// Absent entirely on a node that rendered no text, which is most of a real
/// tree: toolkit layout containers have no label, no description and no value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Text {
    /// The node's label -- what a person reads off the screen, and what a
    /// selector matches on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Supplementary text: a tooltip, a hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The node's contents -- an entry's text, a slider's reading.
    ///
    /// **Absent from every node this build produces**, and worth saying rather
    /// than leaving to be inferred: an accessible object's contents live on
    /// AT-SPI's `Text` and `Value` interfaces, and `perspicax-atspi` reads
    /// neither, so nothing ever fills this in. It is projected because it is
    /// part of the schema and because the day an ingest reads those interfaces
    /// the wire should not have to change -- but until then, absent here means
    /// "not read", never "empty". Measured 2026-09-05: typing into a real GTK
    /// entry through `act` puts the text in the entry and leaves this `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The process that rendered every string above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_by: Option<Provenance>,
}

impl Text {
    /// Everything this node rendered, or `None` if it rendered nothing.
    #[must_use]
    pub fn of(node: &ObservedNode) -> Option<Self> {
        let (label, description, value) = (
            node.node.label().map(Into::into),
            node.node.description().map(Into::into),
            node.node.value().map(Into::into),
        );
        if label.is_none() && description.is_none() && value.is_none() {
            return None;
        }
        Some(Self {
            label,
            description,
            value,
            rendered_by: Provenance::of(&node.origin),
        })
    }
}

/// A rectangle, as an origin and a size.
///
/// `perspicax-node`'s [`Rect`] is kurbo-derived and is two corners; an agent
/// reading JSON wants width and height, and converting once here is cheaper
/// than every client doing the subtraction and one of them getting it
/// backwards.
///
/// **No tool accepts one of these.** Coordinates leave this system and never
/// enter it: a [`Verb`](perspicax_index::Verb) names a control and the
/// compositor supplies the geometry, so an agent that cannot name a pixel
/// cannot name the wrong one. These are here to be reasoned about -- which of
/// two buttons is on the left -- not to be sent back.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<Rect> for Bounds {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x0,
            y: rect.y0,
            width: rect.width(),
            height: rect.height(),
        }
    }
}

/// Why an agent may not act on a node, in a shape a client can branch on.
///
/// `kind` is the machine-readable discriminant and `message` is
/// [`Refusal`]'s own `Display`, so a model gets both without either being a
/// paraphrase of the other. The remaining fields are what that particular
/// refusal carries -- the occluding surface so the agent can raise it, the match
/// count so it can narrow a selector. A refusal that only said "no" would turn a
/// recoverable situation into a retry loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    /// One of `occluded`, `clipped`, `unmapped`, `off_screen`,
    /// `other_workspace`, `inactive_tab`, `not_showing`, `unjudged`,
    /// `unplaced`, `unattributed`, `stale`, `no_capability`,
    /// `ambiguous_selector`, `not_found`, `not_a_window`, `focus_elsewhere`.
    pub kind: &'static str,
    /// The refusal in words.
    pub message: String,
    /// `occluded`: raise this surface and try again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occluded_by: Option<u64>,
    /// `stale`: how many frames of damage the node owes. Zero is a real value --
    /// the accessibility feed said the subtree changed shape and no pixel moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u32>,
    /// `ambiguous_selector`: how many nodes matched. Narrow the selector, or
    /// index into it with `[n]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<usize>,
    /// `other_workspace`: the workspace the window is on, counting from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<u16>,
    /// `inactive_tab`: the tab in front, which has the group's place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_tab: Option<u64>,
    /// `focus_elsewhere`: the window or layer surface holding the keyboard,
    /// where the keys would have gone. Absent when none holds it. `focus` the
    /// node, then type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused: Option<u64>,
}

impl From<&Refusal> for Refused {
    fn from(refusal: &Refusal) -> Self {
        let mut refused = Self {
            kind: match refusal {
                Refusal::Occluded { .. } => "occluded",
                Refusal::Clipped => "clipped",
                Refusal::Unmapped => "unmapped",
                Refusal::OffScreen => "off_screen",
                Refusal::OtherWorkspace { .. } => "other_workspace",
                Refusal::InactiveTab { .. } => "inactive_tab",
                Refusal::NotShowing => "not_showing",
                Refusal::Unjudged => "unjudged",
                Refusal::Unplaced => "unplaced",
                Refusal::Unattributed => "unattributed",
                Refusal::Stale { .. } => "stale",
                Refusal::NoCapability { .. } => "no_capability",
                Refusal::AmbiguousSelector { .. } => "ambiguous_selector",
                Refusal::NotFound => "not_found",
                Refusal::NotAWindow => "not_a_window",
                Refusal::FocusElsewhere { .. } => "focus_elsewhere",
            },
            message: refusal.to_string(),
            occluded_by: None,
            frames: None,
            matches: None,
            workspace: None,
            shown_tab: None,
            focused: None,
        };
        match refusal {
            Refusal::Occluded { by } => refused.occluded_by = Some(by.0),
            Refusal::Stale { frames } => refused.frames = Some(*frames),
            Refusal::AmbiguousSelector { matches } => refused.matches = Some(*matches),
            Refusal::OtherWorkspace { workspace } => refused.workspace = Some(*workspace),
            Refusal::InactiveTab { shown } => refused.shown_tab = Some(shown.0),
            Refusal::FocusElsewhere { focused } => {
                refused.focused = focused.map(|surface| surface.0);
            }
            _ => {}
        }
        refused
    }
}

/// What a toolkit says about a control beyond its role and its label.
///
/// Only what is actually set: a bridge that reports nothing about selection
/// leaves `selected` absent, which is a different statement from "not selected"
/// and is the one AT-SPI is entitled to make. Every field is skipped when it is
/// false or absent, so a plain button serialises to nothing at all and the
/// object itself is omitted -- see [`State::any`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct State {
    /// Unavailable for interaction. Still actable as far as the gate is
    /// concerned -- the compositor will happily click a greyed-out button, and
    /// saying so is more honest than pretending the click was impossible.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub disabled: bool,
    /// The application says this control is not being shown: a menu bar
    /// hidden until Alt is pressed, the page of a tab behind another. Never
    /// actable, and refused as `not_showing`, because whatever a press there
    /// landed on would be something else drawn in the same place.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub hidden: bool,
    /// Input or selection is required.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub required: bool,
    /// Being modified; updates should be withheld until it settles.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub busy: bool,
    /// A modal dialog. Everything behind it is unreachable.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub modal: bool,
    /// Focusable and selectable, but not editable.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub read_only: bool,
    /// More than one descendant may be selected at once.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub multiselectable: bool,
    /// A link that has been followed.
    #[serde(skip_serializing_if = "core::ops::Not::not")]
    pub visited: bool,
    /// `true`, `false` or `mixed` -- a checkbox or a toggle button. Tri-state,
    /// because a partially-checked parent checkbox is a real thing and a `bool`
    /// would have to lie about it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggled: Option<&'static str>,
    /// Expanded or collapsed. Absent means the concept does not apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// Selected or not. Absent means the concept does not apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// `horizontal` or `vertical`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<&'static str>,
}

impl State {
    /// Everything the bridge said about this control.
    #[must_use]
    pub fn of(node: &ObservedNode) -> Self {
        let node = &node.node;
        Self {
            disabled: node.is_disabled(),
            hidden: node.is_hidden(),
            required: node.is_required(),
            busy: node.is_busy(),
            modal: node.is_modal(),
            read_only: node.is_read_only(),
            multiselectable: node.is_multiselectable(),
            visited: node.is_visited(),
            toggled: node.toggled().map(|toggled| match toggled {
                Toggled::False => "false",
                Toggled::True => "true",
                Toggled::Mixed => "mixed",
            }),
            expanded: node.is_expanded(),
            selected: node.is_selected(),
            orientation: node.orientation().map(|orientation| match orientation {
                Orientation::Horizontal => "horizontal",
                Orientation::Vertical => "vertical",
            }),
        }
    }

    /// Whether anything at all is set, so an empty one can be left off the wire.
    #[must_use]
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

/// One node, as an agent sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    /// The stable id. Hand it back to `observe` to read this subtree, and never
    /// expect it to name a different widget: ids are minted once and retired,
    /// never reused.
    pub node: u64,
    /// The role, spelled exactly as a selector spells it. `button:Cancel`
    /// matches a node whose `role` reads `Button`, case-insensitively.
    pub role: String,
    /// Every string this node rendered, and who rendered it. See [`Text`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_text: Option<Text>,
    /// The compositor surface this node was drawn on, once a host has joined
    /// it. Absent means no compositor has attributed it, and it is not actable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<u64>,
    /// **Window-relative**: from the top left of the window's own `bounds` in
    /// `window_list`, inside any shadow, whichever origin the toolkit measures
    /// from. Compare it against that window's bounds, never another's. Absent
    /// when the bridge reported no extents, when no compositor has
    /// attributed the node, and when its window reported no extents of its
    /// own to measure from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,
    /// This node's children, in the order the node itself lists them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<u64>,
    /// Whether `act` would be allowed on this node right now.
    pub actable: bool,
    /// Why not, when `actable` is false. The gate's answer, given before the
    /// agent spends a call on finding out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<Refused>,
    /// What the toolkit says about this control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
}

impl Node {
    /// Project one cached node, asking the gate what it would say.
    ///
    /// The gate is consulted here rather than reimplemented, which is the same
    /// rule the act path follows: `perspicax-index` decides actability once,
    /// and everything that reports on it asks. A projection with its own
    /// opinion would eventually disagree with the thing that actually refuses.
    #[must_use]
    pub fn of(index: &Index, id: NodeId) -> Option<Self> {
        let node = index.get(id)?;
        let state = State::of(node);
        let refused = index.actable(id).err();
        Some(Self {
            node: id.0,
            role: format!("{:?}", node.node.role()),
            untrusted_text: Text::of(node),
            surface: node.surface.map(|surface| surface.0),
            bounds: index.window_bounds(id).map(Into::into),
            children: node.node.children().iter().map(|child| child.0).collect(),
            actable: refused.is_none(),
            refused: refused.as_ref().map(Into::into),
            state: state.any().then_some(state),
        })
    }
}

/// One window: a compositor surface, and the accessible tree drawn on it.
///
/// Surfaces come first and nodes second, which is the opposite of how every
/// accessibility tool works and is the point. A surface that no bridge
/// describes still appears here, with `node` absent and `nodes` zero -- that is
/// a window rendering content nothing can explain, which is precisely the case
/// an agent needs told about rather than quietly omitted.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    /// The compositor surface.
    pub surface: u64,
    /// What it is: `window`, an application's; `layer`, part of the desk
    /// such as a panel, a wallpaper or a menu; or `lock_cover`, a screen
    /// locker's. `window_close` and `tab_forward` are for windows only.
    pub kind: &'static str,
    /// For a layer surface, which layer it stacks in: `background` and
    /// `bottom` under every window, `top` and `overlay` over them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<&'static str>,
    /// For a layer surface, the namespace its client gave it, under a key
    /// that says who chose it, as the title is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_namespace: Option<String>,
    /// The window's own accessible node, the root of what is drawn on it.
    /// Pass it to `observe` as `root` to read this window and nothing else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<u64>,
    /// How many cached nodes sit on this surface.
    pub nodes: usize,
    /// Whether it is on screen at all.
    pub mapped: bool,
    /// Where it is, in the output's coordinate space. The only global geometry
    /// this system reports.
    pub bounds: Bounds,
    /// Who drew it. Present here at the top level rather than only beside the
    /// title, because a surface with no title and no accessible nodes is
    /// exactly the surface whose provenance matters most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_by: Option<Provenance>,
    /// The title the client set, under a key that says who set it. A window
    /// title is a string an untrusted process chose; `rendered_by` above names
    /// the process that chose it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_title: Option<String>,
    /// How many frames of damage this surface has ever taken, what it drew
    /// into its subsurfaces included: one for each commit that brought a new
    /// buffer or named damage. Read it as a rate, not a total: an idle GTK
    /// window repaints about forty times a second and an idle Qt one about
    /// once every two.
    pub damage_frames: u64,
    /// The app id the client set, under a key that says who set it, as the
    /// title is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_app_id: Option<String>,
    /// The workspace it belongs to, counting from 1, whether or not that
    /// workspace is showing. Absent for a window on every workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<u16>,
    /// Its tab group, when it is in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tabs: Option<Tabs>,
    /// How opaque it is drawn, in percent, when the person has made it less
    /// than wholly so; absent when it is opaque. What is behind it then shows
    /// through in a picture of the monitor, and is still covered: `observe`
    /// and `act` treat the window as solid, by the person's decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<u8>,
}

/// A window's tab group: every tab, and which is in front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tabs {
    /// Every tab's surface, in tab order.
    pub members: Vec<u64>,
    /// The tab in front, which has the group's place on screen. Any other
    /// can be brought forward with `tab_forward`.
    pub front: u64,
}

impl Window {
    /// Every surface the host has published, with whatever the index knows
    /// about it.
    ///
    /// Ordered by surface id, which is mint order, which is the order the
    /// windows appeared. Bottom-to-top z-order is a different question and is
    /// the host's to answer; this is a list, not a stack.
    #[must_use]
    pub fn all(index: &Index, facts: &HostFacts) -> Vec<Self> {
        let order = index.preorder();
        let mut windows: Vec<Self> = facts
            .surfaces()
            .iter()
            .map(|surface| Self::one(index, surface, &order))
            .collect();
        windows.sort_by_key(|window| window.surface);
        windows
    }

    fn one(index: &Index, facts: &SurfaceFacts, order: &[NodeId]) -> Self {
        let on_this_surface = |id: &&NodeId| {
            index
                .get(**id)
                .is_some_and(|node| node.surface == Some(facts.id))
        };
        let (kind, layer, namespace) = match &facts.kind {
            SurfaceKind::Window => ("window", None, None),
            SurfaceKind::Layer { layer, namespace } => {
                ("layer", Some(layer.name()), Some(namespace.clone()))
            }
            SurfaceKind::LockCover => ("lock_cover", None, None),
        };
        Self {
            surface: facts.id.0,
            kind,
            layer,
            untrusted_namespace: namespace,
            // The index's own record of the window node: the one its nodes'
            // window bounds are measured from, so that what `observe` shows
            // of this window and where it says its nodes are agree.
            node: index.window_node(facts.id).map(|id| id.0),
            nodes: order.iter().filter(on_this_surface).count(),
            mapped: facts.mapped,
            bounds: facts.geometry.into(),
            rendered_by: Provenance::of(&facts.origin),
            untrusted_title: facts.title.clone(),
            damage_frames: facts.damage_generation,
            untrusted_app_id: facts.app_id.clone(),
            workspace: facts.workspace,
            tabs: (!facts.tabs.is_empty()).then(|| Tabs {
                members: facts.tabs.iter().map(|tab| tab.0).collect(),
                front: facts.behind_tab.unwrap_or(facts.id).0,
            }),
            opacity: facts.drawn_opacity,
        }
    }
}

/// What the pixels did while an act was given time to have an effect.
///
/// Not a verdict, and deliberately not a `bool`. An idle `gtk4-widget-factory`
/// damages its whole window about forty times a second, so "the surface
/// changed" is true of every act on it and means nothing; Qt's gallery manages
/// about one repaint every two seconds and so answers the question rather well.
/// A receipt that flattened those into "it worked" would be confidently wrong on
/// one of the two toolkits this project tests against.
///
/// `quiet` is the strong answer: nothing changed at all, on a surface that was
/// asked to change. Weigh `on_target` against `frames` and against what that
/// surface does when nothing is happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Damage {
    /// `quiet`, `elsewhere`, or `on_target`.
    pub witness: &'static str,
    /// How many frames of damage arrived while watching.
    pub frames: u64,
    /// How long the act was given.
    pub window_ms: u64,
}

/// What happened when an agent acted.
///
/// Every field is something only the acting path knew and nobody can recover
/// afterwards. **There is no `success`.** See [`Damage`] for why a boolean here
/// would be a confident lie on at least one real toolkit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Acted {
    /// The selector as the agent wrote it.
    pub selector: String,
    /// The node it resolved to. Act on it again without re-resolving.
    pub node: u64,
    /// The surface that node was drawn on.
    pub surface: u64,
    /// Who owns that surface, from its connection credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_by: Option<Provenance>,
    /// `click`, `type`, `scroll` or `focus`.
    pub verb: &'static str,
    /// The round trip to the compositor's thread and back, not counting the
    /// wait for damage.
    pub dispatch_ms: f64,
    /// Which surface held keyboard focus before the act.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_before: Option<u64>,
    /// And after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_after: Option<u64>,
    /// Whether focus landed on the surface acted on. Not "did it work": plenty
    /// of controls take a click without taking focus.
    pub focus_landed: bool,
    /// What the pixels did.
    pub damage: Damage,
}

impl From<&Receipt> for Acted {
    fn from(receipt: &Receipt) -> Self {
        Self {
            selector: receipt.selector.clone(),
            node: receipt.node.0,
            surface: receipt.surface.0,
            rendered_by: Provenance::of(&receipt.origin),
            verb: receipt.verb.name(),
            dispatch_ms: receipt.dispatch.as_secs_f64() * 1000.0,
            focus_before: receipt.focus_before.map(|surface: SurfaceId| surface.0),
            focus_after: receipt.focus_after.map(|surface: SurfaceId| surface.0),
            focus_landed: receipt.focus_landed(),
            damage: Damage {
                witness: match receipt.damage {
                    DamageWitness::Quiet => "quiet",
                    DamageWitness::Elsewhere { .. } => "elsewhere",
                    DamageWitness::OnTarget { .. } => "on_target",
                },
                frames: receipt.damage.frames(),
                window_ms: u64::try_from(receipt.damage_window.as_millis()).unwrap_or(u64::MAX),
            },
        }
    }
}

/// What became of a window an agent closed or brought forward.
///
/// No `success`, for the reason [`Acted`] has none: `witness` is what was
/// seen once the window had been given `window_ms` to react.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowActed {
    /// The window acted on.
    pub surface: u64,
    /// Who owns it, from its connection credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_by: Option<Provenance>,
    /// `close` or `forward`.
    pub verb: &'static str,
    /// The round trip to the compositor's thread and back.
    pub dispatch_ms: f64,
    /// Which surface held keyboard focus before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_before: Option<u64>,
    /// And after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_after: Option<u64>,
    /// `gone`, `still_open`, `in_front` or `still_behind`.
    pub witness: &'static str,
    /// `still_open`: windows of the same process that appeared meanwhile --
    /// a dialog asking about unsaved work, typically. Observe it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub appeared: Vec<u64>,
    /// `still_behind`: the tab still in front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_tab: Option<u64>,
    /// How long the window was given to react.
    pub window_ms: u64,
}

impl From<&WindowReceipt> for WindowActed {
    fn from(receipt: &WindowReceipt) -> Self {
        let (witness, appeared, shown_tab) = match &receipt.witness {
            WindowWitness::Gone => ("gone", Vec::new(), None),
            WindowWitness::StillOpen { appeared } => (
                "still_open",
                appeared.iter().map(|surface| surface.0).collect(),
                None,
            ),
            WindowWitness::InFront => ("in_front", Vec::new(), None),
            WindowWitness::StillBehind { shown } => ("still_behind", Vec::new(), Some(shown.0)),
        };
        Self {
            surface: receipt.surface.0,
            rendered_by: Provenance::of(&receipt.origin),
            verb: receipt.verb.name(),
            dispatch_ms: receipt.dispatch.as_secs_f64() * 1000.0,
            focus_before: receipt.focus_before.map(|surface| surface.0),
            focus_after: receipt.focus_after.map(|surface| surface.0),
            witness,
            appeared,
            shown_tab,
            window_ms: u64::try_from(receipt.window.as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// One surface in a picture: where it is in it, and who drew it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pictured {
    pub surface: u64,
    /// Where it is in the picture, in logical units from the top left;
    /// multiply by `scale` for pixels.
    pub bounds: Bounds,
    /// Who drew it, from its connection credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_by: Option<Provenance>,
    /// The title its client set. Absent for a surface painted over: the
    /// picture does not show it, and the account of it does not either.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_title: Option<String>,
}

/// A picture's account of itself, beside the image.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Screenshot {
    pub captured: bool,
    /// In pixels.
    pub width: u32,
    pub height: u32,
    /// Pixels per logical unit.
    pub scale: f64,
    /// The monitor it is of, for a monitor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// For a window: whether the person can see it now. A window's picture
    /// is of the window alone, so it shows the same whether anything is over
    /// it, or it is on another workspace, or minimized; this says which.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapped: Option<bool>,
    /// Every surface in the picture, front first.
    pub drawn: Vec<Pictured>,
    /// Surfaces painted over, because the agent holds no consent for whoever
    /// drew them. The picture shows a grey block where each one is.
    pub redacted: Vec<Pictured>,
    /// Nodes under damage no accessibility event explained: what a picture is
    /// for. Zero means the accessible tree already describes everything that
    /// changed.
    pub unexplained: usize,
}

impl Screenshot {
    #[must_use]
    pub fn of(
        shot: &Shot,
        facts: &HostFacts,
        window: Option<SurfaceId>,
        unexplained: usize,
    ) -> Self {
        let pictured = |drawn: &Drawn, shown: bool| Pictured {
            surface: drawn.surface.0,
            bounds: drawn.rect.into(),
            rendered_by: Provenance::of(&drawn.origin),
            untrusted_title: shown
                .then(|| facts.surface(drawn.surface)?.title.clone())
                .flatten(),
        };
        Self {
            captured: true,
            width: shot.width,
            height: shot.height,
            scale: shot.scale,
            output: shot.output.clone(),
            mapped: window.and_then(|id| Some(facts.surface(id)?.mapped)),
            drawn: shot
                .drawn
                .iter()
                .map(|drawn| pictured(drawn, true))
                .collect(),
            redacted: shot
                .redacted
                .iter()
                .map(|drawn| pictured(drawn, false))
                .collect(),
            unexplained,
        }
    }
}

/// A picture as a PNG.
///
/// # Errors
///
/// When the pixels are not the size the picture says.
pub fn png(shot: &Shot) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, shot.width, shot.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(&shot.rgba)
        .map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(encoded)
}

/// Something that changed since the last time anyone asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Changed {
    /// `added`, `updated`, `removed`, or `invalidated`.
    pub change: &'static str,
    /// The node it happened to. For `invalidated` this is the root of a subtree
    /// that must be re-read before anything in it is trusted.
    pub node: u64,
}

impl From<&Delta> for Changed {
    fn from(delta: &Delta) -> Self {
        match delta {
            Delta::Added { id } => Self {
                change: "added",
                node: id.0,
            },
            Delta::Updated { id } => Self {
                change: "updated",
                node: id.0,
            },
            Delta::Removed { id } => Self {
                change: "removed",
                node: id.0,
            },
            Delta::Invalidated { root } => Self {
                change: "invalidated",
                node: root.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{BURIED, CANCEL, FRAME, OVERLAY, WINDOW, facts, index};

    /// The read-path security posture, as one assertion: a string an
    /// application rendered is reachable only through a key that says so, and
    /// the process that rendered it is in the same object rather than in a
    /// preamble the reader has to have kept.
    #[test]
    fn every_string_arrives_under_a_key_that_names_it_untrusted() {
        let node = Node::of(&index(), CANCEL).expect("the fixture has it");
        let text = node.untrusted_text.expect("Cancel has a label");

        assert_eq!(text.label.as_deref(), Some("Cancel"));
        assert_eq!(text.description.as_deref(), Some("Discard the changes"));
        let rendered_by = text.rendered_by.expect("the window is attributed");
        assert_eq!(rendered_by.pid, 4242);
        assert_eq!(
            rendered_by.exe.as_deref(),
            Some("/usr/bin/gtk4-widget-factory")
        );

        // And the whole point of the key: `label` is not a field of the node.
        let json = serde_json::to_value(Node::of(&index(), CANCEL).unwrap()).unwrap();
        assert!(json.get("label").is_none());
        assert_eq!(json["untrusted_text"]["label"], "Cancel");
    }

    /// Most of a real tree is toolkit scaffolding with nothing to say, and
    /// carrying an empty object for each of those nodes would be several
    /// kilobytes of context spent on saying nothing.
    #[test]
    fn a_node_that_rendered_nothing_carries_no_text_at_all() {
        let mut bare = ObservedNode::unjoined(
            NodeId(9),
            perspicax_node::Node::new(perspicax_node::Role::Group),
        );
        bare.origin = crate::fixture::origin();
        assert_eq!(Text::of(&bare), None);
    }

    /// The gate is asked, not reimplemented, and its answer travels with the
    /// node -- so an agent sees the occlusion before it spends a call finding
    /// out about it.
    #[test]
    fn a_covered_node_says_so_and_names_the_surface_in_the_way() {
        let index = index();

        let cancel = Node::of(&index, CANCEL).expect("the fixture has it");
        assert!(cancel.actable);
        assert_eq!(cancel.refused, None);

        let buried = Node::of(&index, BURIED).expect("the fixture has it");
        assert!(!buried.actable);
        let refused = buried.refused.expect("it is covered");
        assert_eq!(refused.kind, "occluded");
        assert_eq!(refused.occluded_by, Some(OVERLAY.0));
        assert_eq!(refused.message, "node occluded by surface 2");
    }

    /// Each refusal carries what recovery would need, and only that: an
    /// occlusion has no match count and an ambiguity has no surface. A refusal
    /// that answered every question with `null` would be a schema, not an
    /// answer.
    #[test]
    fn a_refusal_carries_its_own_remedy_and_no_other() {
        let ambiguous = Refused::from(&Refusal::AmbiguousSelector { matches: 3 });
        assert_eq!(ambiguous.kind, "ambiguous_selector");
        assert_eq!(ambiguous.matches, Some(3));
        assert_eq!(ambiguous.occluded_by, None);

        let stale = Refused::from(&Refusal::Stale { frames: 0 });
        assert_eq!(stale.frames, Some(0));
        assert_eq!(
            stale.message,
            "node's subtree was invalidated and has not been re-read"
        );

        let json = serde_json::to_value(Refused::from(&Refusal::NotFound)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "not_found",
            "message": "selector matched no nodes"})
        );
    }

    /// Nothing to raise and nothing to count: the remedy is in the words, and
    /// the wire carries no field another refusal would fill.
    #[test]
    fn not_showing_goes_on_the_wire_as_its_kind_and_its_message_alone() {
        assert_eq!(
            serde_json::to_value(Refused::from(&Refusal::NotShowing)).unwrap(),
            serde_json::json!({"kind": "not_showing",
            "message": "node's application reports it as not being shown"})
        );
    }

    /// Issue #51. A node its application will not place is not one that has
    /// not been judged *yet*, and the wire says so, so that an agent stops
    /// waiting for a verdict that cannot come.
    #[test]
    fn unplaced_goes_on_the_wire_as_its_kind_and_its_message_alone() {
        assert_eq!(
            serde_json::to_value(Refused::from(&Refusal::Unplaced)).unwrap(),
            serde_json::json!({"kind": "unplaced",
            "message": "node's application gives no bounds for it, \
                        so where it is drawn cannot be judged"})
        );
    }

    /// Issue #32 put `"actable": true` beside `"state": {"hidden": true}` on
    /// one node. A node its application says is not shown is now reported
    /// the way the gate treats it, so the two can no longer disagree.
    #[test]
    fn a_node_its_application_hides_is_reported_hidden_and_refused_together() {
        let mut frame = perspicax_node::Node::new(perspicax_node::Role::Window);
        frame.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
        frame.set_children(vec![NodeId(2)]);
        let mut menu = perspicax_node::Node::new(perspicax_node::Role::Menu);
        menu.set_label("File");
        menu.set_bounds(Rect::new(10.0, 10.0, 50.0, 36.0));
        menu.set_hidden();

        let mut index = Index::new();
        index.ingest_snapshot([
            ObservedNode::unjoined(NodeId(1), frame),
            ObservedNode::unjoined(NodeId(2), menu),
        ]);
        index.join_subtree(NodeId(1), WINDOW, &crate::fixture::origin());
        index.judge(&facts());

        let file = Node::of(&index, NodeId(2)).expect("it was ingested");
        assert!(!file.actable);
        assert_eq!(
            file.refused.map(|refused| refused.kind),
            Some("not_showing")
        );
        assert!(file.state.is_some_and(|state| state.hidden));
    }

    /// A keyboard held elsewhere names where the keys would have gone, so the
    /// agent can see it in `window_list`; with nothing holding it there is
    /// nothing to name, and no `null` stands in for it.
    #[test]
    fn focus_elsewhere_names_the_focused_surface_and_omits_it_when_none_holds_it() {
        let elsewhere = Refusal::FocusElsewhere {
            focused: Some(SurfaceId(3)),
        };
        assert_eq!(
            serde_json::to_value(Refused::from(&elsewhere)).unwrap(),
            serde_json::json!({"kind": "focus_elsewhere",
            "message": "keyboard focus is on surface 3, not in node's window",
            "focused": 3})
        );

        let nowhere = Refusal::FocusElsewhere { focused: None };
        assert_eq!(
            serde_json::to_value(Refused::from(&nowhere)).unwrap(),
            serde_json::json!({"kind": "focus_elsewhere",
            "message": "no window holds keyboard focus"})
        );
    }

    /// The claim this whole project is built to make, in the smallest form it
    /// takes: a surface nothing describes is still reported, because "there is
    /// a window here and no accessibility tree explains it" is the answer, not
    /// the absence of one.
    #[test]
    fn a_surface_no_bridge_describes_is_still_a_window() {
        let windows = Window::all(&index(), &facts());
        assert_eq!(windows.len(), 2);

        let described = &windows[0];
        assert_eq!(described.surface, WINDOW.0);
        assert_eq!(described.node, Some(FRAME.0));
        assert_eq!(described.nodes, 3);
        assert_eq!(described.untrusted_title.as_deref(), Some("Widget Factory"));
        assert_eq!(described.rendered_by.as_ref().map(|by| by.pid), Some(4242));
        assert_eq!(described.damage_frames, 41);

        let undescribed = &windows[1];
        assert_eq!(undescribed.surface, OVERLAY.0);
        assert_eq!(undescribed.node, None);
        assert_eq!(undescribed.nodes, 0);
        assert_eq!(undescribed.untrusted_title, None);
        // Still attributed: the compositor knows who drew it even though
        // nothing can say what is in it. That is the pair of facts that makes
        // this worth reporting rather than skipping.
        assert_eq!(
            undescribed.rendered_by.as_ref().map(|by| by.pid),
            Some(5150)
        );
    }

    /// A window the person made see-through says how opaque it is drawn, and
    /// one that is opaque says nothing: no `opacity: 100` on every window.
    #[test]
    fn a_see_through_window_says_how_opaque_it_is_drawn_and_an_opaque_one_is_silent() {
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(WINDOW, Rect::new(0.0, 0.0, 400.0, 300.0)).drawn_at(77),
                SurfaceFacts::new(OVERLAY, Rect::new(100.0, 100.0, 300.0, 250.0)),
            ],
            1,
        );
        let windows = Window::all(&index(), &facts);
        let wire = serde_json::to_value(&windows).unwrap();
        assert_eq!(wire[0]["opacity"], 77);
        assert!(wire[1].get("opacity").is_none(), "{}", wire[1]);
    }

    /// The window node is the shallowest node on its surface, and finding it
    /// that way rather than by role is deliberate: GTK offers a `frame` and Qt
    /// offers a `dialog`, and a search for `Role::Window` finds one of them.
    #[test]
    fn a_windows_node_is_the_shallowest_one_drawn_on_it() {
        let windows = Window::all(&index(), &facts());
        assert_eq!(windows[0].node, Some(FRAME.0));
    }

    /// `Rect` is kurbo-derived and is two corners. Every client would otherwise
    /// do this subtraction, and one of them would do it backwards.
    #[test]
    fn two_corners_become_an_origin_and_a_size() {
        let bounds = Bounds::from(Rect::new(10.0, 20.0, 110.0, 70.0));
        assert_eq!((bounds.x, bounds.y), (10.0, 20.0));
        assert_eq!((bounds.width, bounds.height), (100.0, 50.0));
    }

    /// A plain control serialises to no `state` key at all, and one the bridge
    /// said something about carries only what it said. AT-SPI's silence about
    /// selection is a different statement from "not selected", and flattening
    /// the two would put a claim on the wire that nothing made.
    #[test]
    fn state_reports_what_the_bridge_said_and_not_what_it_did_not() {
        let index = index();
        assert_eq!(Node::of(&index, CANCEL).unwrap().state, None);

        let buried = Node::of(&index, BURIED).unwrap();
        let state = buried.state.expect("it is disabled");
        assert!(state.disabled);
        assert_eq!(state.selected, None);
        assert_eq!(state.toggled, None);

        let json = serde_json::to_value(state).unwrap();
        assert_eq!(json, serde_json::json!({"disabled": true}));
    }

    /// There is no `success`, and a test says so rather than a comment, because
    /// the field is exactly what a future reader would think was missing.
    #[test]
    fn a_receipt_reports_evidence_and_never_a_verdict() {
        use perspicax_index::{DamageWitness, Verb};
        use std::time::Duration;

        let receipt = Receipt {
            selector: "button:Cancel".to_owned(),
            node: CANCEL,
            surface: WINDOW,
            origin: crate::fixture::origin(),
            verb: Verb::Focus,
            dispatch: Duration::from_micros(420),
            focus_before: None,
            focus_after: Some(WINDOW),
            damage: DamageWitness::OnTarget { frames: 3 },
            damage_window: Duration::from_millis(200),
        };

        let acted = Acted::from(&receipt);
        assert_eq!(acted.verb, "focus");
        assert!((acted.dispatch_ms - 0.42).abs() < 1e-9);
        assert!(acted.focus_landed);
        assert_eq!(acted.damage.witness, "on_target");
        assert_eq!(acted.damage.frames, 3);
        assert_eq!(acted.damage.window_ms, 200);

        let json = serde_json::to_value(&acted).unwrap();
        assert!(json.get("success").is_none());
        // And focus_before is absent rather than null: nothing held focus, and
        // a `null` would invite a reader to treat it as a surface id of none.
        assert!(json.get("focus_before").is_none());
    }

    #[test]
    fn a_change_is_named_by_what_happened_to_the_node() {
        assert_eq!(
            Changed::from(&Delta::Invalidated { root: FRAME }),
            Changed {
                change: "invalidated",
                node: FRAME.0
            }
        );
        assert_eq!(Changed::from(&Delta::Added { id: CANCEL }).change, "added");
    }
}
