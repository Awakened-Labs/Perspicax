//! What only a host knows, reduced to plain data -- and the judgement that data
//! makes possible.
//!
//! This module is the reason [`HostView`](crate::HostView) is a small trait. A
//! compositor answers three questions, and only one of them is genuinely
//! compositor-shaped: dispatching input. The other two -- who owns this
//! surface, and can this rect be seen -- are *arithmetic over facts a
//! compositor happens to hold*. Keeping the arithmetic here rather than inside
//! `perspicax-compositor` buys two things:
//!
//! - **The hardest correctness in the system becomes testable with no Wayland
//!   anywhere.** Occlusion is where a wrong answer is worst, because it is not
//!   a crash: it is a plausible box in the wrong place, which an agent then
//!   clicks. Every case in the policy below is a unit test over a [`HostFacts`]
//!   built by hand.
//! - **A second host is a data producer, not a second implementation.** A GNOME
//!   extension or a KWin plugin has to *fill in* [`HostFacts`]; it does not get
//!   to re-decide what `Occluded` means. That is the same argument that puts
//!   the refusal gate in this crate rather than in the MCP server.
//!
//! # Four coordinate spaces, and who measures the way between them
//!
//! - **Node space** is what an accessibility bridge reports: window-relative,
//!   from whatever origin its toolkit calls the window's, and never global.
//!   See [`ObservedNode::node_space_bounds`](perspicax_node::ObservedNode::node_space_bounds).
//! - **Window space** is relative to the origin of the window geometry the
//!   host placed: the visible frame, inside any client-side shadow. Every
//!   toolkit's node space is brought into it, and it is what a verdict, an
//!   act and a receipt are computed in.
//! - **Surface space** is what the Wayland protocol delivers regions in: an
//!   opaque region's rectangles and a commit's damage are surface-local.
//! - **Global space** is the host's own, and the only one in which two windows
//!   can be compared at all.
//!
//! [`SurfaceFacts::geometry`] places window space in global space, and
//! [`SurfaceFacts::buffer_origin`] places surface space. Both are the host's
//! to say, because the host placed them.
//!
//! The way from node space to window space is *not* the host's to say, and
//! this crate measures it from the join: the window node's own extents
//! origin, which [`Index::window_origin`](crate::Index::window_origin)
//! subtracts. **It is zero for GTK 4.18.6 and Qt 6.8.2** (measured 2026-09-04,
//! both under client-side decoration: each window node starts at `0,0`, and
//! GTK's reported size, 1666x881, is exactly the geometry this compositor
//! placed, `32,32 -> 1698,913`). **It is the shadow for Firefox 148**, which
//! measures from its buffer: its window node starts at `(26, 23)`, and a
//! click aimed as though it were zero landed beside every button it was
//! sent to (issue #45). A host cannot tell which convention a toolkit uses;
//! the toolkit's own window node can.
//!
//! # The occlusion policy, stated once
//!
//! `wl_surface.set_opaque_region` is a client *hint*, and an optional one. A
//! client may declare where it is opaque, may declare nothing, or may declare
//! something stale. So the policy is:
//!
//! **A surface that declares an opaque region is believed about where it is
//! opaque. A surface that declares nothing is not guessed at, and occludes.**
//!
//! Overlap without proof of transparency is [`Visibility::Occluded`], naming
//! the surface in the way so an agent can raise it and retry. The alternative --
//! assume anything undeclared is see-through -- restores exactly the failure
//! this project exists to remove: a covered node reported as visible. Because
//! that policy can also refuse too much, every verdict reached this way is
//! counted ([`Judgement::unproven`], [`Tally::unproven`]), so a toolkit that
//! declares nothing shows up as a number rather than as a mystery.
//!
//! # What this module deliberately does not know
//!
//! Input regions. "Can a human see this" and "would a click at this point land
//! on this surface" are different questions, and a click-through overlay
//! answers them differently: it is visible *and* it does not take the click.
//! [`Visibility`] answers the first. The second belongs to the act path in M3,
//! where the host can be asked which surface a point resolves to -- and where
//! being wrong is refused rather than reported.

use std::time::Instant;

use perspicax_node::{Origin, Rect, SurfaceId, Vec2, Visibility};

use crate::{Consent, join::SurfaceClaim};

/// One surface, as its host currently sees it.
///
/// What sort of surface a [`SurfaceFacts`] describes.
///
/// An application's window is what most of this crate is about. A desktop
/// also has surfaces no application window owns: the panel along the bottom,
/// the wallpaper behind everything, a launcher or a menu over the top, all
/// layer-shell surfaces, and the cover a screen locker puts over every
/// monitor. An agent listing the desk needs to tell them apart, and the join
/// needs something other than a title to tell one layer surface from another.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SurfaceKind {
    /// An application's toplevel: an xdg-shell window, or an X11 one.
    #[default]
    Window,
    /// A layer-shell surface: a panel, a wallpaper, a launcher, a menu.
    Layer {
        /// Which layer it stacks in.
        layer: Layer,
        /// The namespace its client gave it, as `waybar` or
        /// `perspicax-panel-DP-1`: a string the client chose, like a title.
        namespace: String,
    },
    /// What a screen locker drew over a monitor while the session is locked.
    LockCover,
}

/// The four layers of `wlr-layer-shell`, bottom to top. Windows stack between
/// [`Layer::Bottom`] and [`Layer::Top`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// Under everything: a wallpaper.
    Background,
    /// Over the wallpaper and under every window: desktop icons.
    Bottom,
    /// Over the windows: a panel.
    Top,
    /// Over everything, fullscreen windows included: a launcher, a menu.
    Overlay,
}

impl Layer {
    /// Its name as `wlr-layer-shell` spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Bottom => "bottom",
            Self::Top => "top",
            Self::Overlay => "overlay",
        }
    }
}

/// Built by a host on every change it notices and published as a whole
/// [`HostFacts`]; never mutated in place by anything above the host. Every
/// default is the fail-closed one, so a producer that forgets a field cannot
/// make a node clickable by omission -- the same rule
/// [`check_actable`](crate::check_actable) enforces one layer up.
///
/// `PartialEq` and not `Eq`, because geometry is floating point. Two sets of
/// facts can be compared for a test; asserting that comparison is an
/// equivalence relation would be a claim about floats that is not true.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceFacts {
    /// The host's own handle for this surface.
    pub id: SurfaceId,
    /// Whether it is on screen at all. An unmapped surface is a closed menu or
    /// a hidden window, and nothing on it can be seen or acted on.
    pub mapped: bool,
    /// Where the surface sits in global space, as the host places it: the
    /// window geometry, which is window space's origin.
    pub geometry: Rect,
    /// Where surface-local `(0, 0)` sits in global space.
    ///
    /// Usually `geometry`'s own origin and *not* under client-side decoration,
    /// where the buffer starts at the outside of the shadow and the window
    /// geometry starts at the visible frame. Opaque regions arrive in
    /// surface-local coordinates, so getting this wrong offsets every occlusion
    /// test by the shadow margin -- which is precisely the size of error that
    /// produces a confident wrong answer rather than an obvious one.
    pub buffer_origin: Vec2,
    /// Where the client says it is opaque, in surface-local coordinates.
    ///
    /// `None` is materially different from `Some(vec![])`: the first is a
    /// client that never declared anything, the second is one that declared
    /// itself entirely transparent. The policy treats them differently, which
    /// is the whole reason this is an `Option` rather than an empty `Vec`.
    pub opaque: Option<Vec<Rect>>,
    /// Who owns the client that drew this surface, from its credentials.
    pub origin: Origin,
    /// The title the client set on this toplevel, if it set one.
    ///
    /// Here rather than fetched separately because a published snapshot is the
    /// only channel across the thread boundary, and a join assembled from two
    /// channels would be correlating a title from one instant against a z-order
    /// from another.
    pub title: Option<String>,
    /// When the host last gave this surface keyboard focus.
    pub focused_at: Option<Instant>,
    /// How many frames of damage this surface has taken, its subsurfaces'
    /// included: one for each commit that brought a new buffer or named
    /// damage, however many of its tree's surfaces that commit changed.
    /// Monotonic; the index compares it against what it has reconciled to
    /// decide staleness.
    pub damage_generation: u64,
    /// Where recent damage landed, oldest first, in this surface's own
    /// surface-local coordinates, each tagged with the generation it arrived
    /// at. Damage a subsurface took is moved into them by where the
    /// subsurface sits, so what a client drew into one -- Firefox draws every
    /// page that way -- lands where it shows.
    ///
    /// # Why the extent matters and a counter alone will not do
    ///
    /// Measured 2026-09-04: `gtk4-widget-factory` sitting idle with nobody
    /// touching it damages its window about **41 times a second**, while Qt's
    /// widget gallery manages about one every two seconds. A rule that made any
    /// unreconciled damage stale the whole window would therefore refuse every
    /// node in a GTK application permanently -- and no re-read is fast enough
    /// to catch up, because reading that tree costs 62 ms at best.
    ///
    /// So staleness is scoped to where the pixels actually changed. A spinner
    /// repainting its own corner does not make the Cancel button across the
    /// window unsafe to click, and knowing the difference needs the damage
    /// regions, which is information only a compositor has.
    ///
    /// Bounded, because it is a history and not a log. When it no longer
    /// reaches back to what a reader reconciled, the answer is the whole
    /// surface -- see [`SurfaceFacts::damage_since`].
    pub damage: Vec<(u64, Rect)>,
    /// The workspace this window is on, numbered from 1, when that is not a
    /// workspace being shown. The window is unmapped then, and saying where
    /// it is turns a refusal an agent can do nothing about into one it can:
    /// the window is not gone, it is on workspace 3.
    pub off_workspace: Option<u16>,
    /// The tab showing in front of this one, when this window is a tab behind
    /// it in a tab group. The window is unmapped then, and as with a
    /// workspace, saying which tab is in front tells an agent where it went.
    pub behind_tab: Option<SurfaceId>,
    /// What the host drew around this surface, in global space: a titlebar
    /// and a border, outside `geometry`.
    ///
    /// No client drew these pixels, so no client's opaque region can speak
    /// for them, and they are opaque by construction: the host drew them
    /// solid. A node of another window under a titlebar is therefore covered
    /// as a matter of proof, not policy. Empty for a surface that draws its
    /// own frame, or has none.
    pub frame: Vec<Rect>,
    /// The app id the client set (`app_id` under Wayland, the class under
    /// X11): a string it chose for itself, like the title.
    pub app_id: Option<String>,
    /// The tab group this window is in, in tab order, or empty when it is in
    /// none. Which of them is in front is whichever is not `behind_tab`.
    pub tabs: Vec<SurfaceId>,
    /// The workspace this window belongs to, numbered from 1, whether or not
    /// it is showing. `None` for a window on every workspace, or one no
    /// workspace has been decided for.
    pub workspace: Option<u16>,
    /// Whether this is an application's window, a layer-shell surface, or a
    /// lock cover.
    pub kind: SurfaceKind,
}

impl SurfaceFacts {
    /// A mapped surface at `geometry`, with nothing else claimed about it: no
    /// declared opacity, no shadow, no attributed origin, and no damage. Every
    /// builder below moves one of those away from its default.
    #[must_use]
    pub fn new(id: SurfaceId, geometry: Rect) -> Self {
        Self {
            id,
            mapped: true,
            geometry,
            buffer_origin: Vec2::new(geometry.x0, geometry.y0),
            opaque: None,
            origin: Origin::Unattributed,
            title: None,
            focused_at: None,
            damage_generation: 0,
            damage: Vec::new(),
            off_workspace: None,
            behind_tab: None,
            frame: Vec::new(),
            app_id: None,
            tabs: Vec::new(),
            workspace: None,
            kind: SurfaceKind::Window,
        }
    }

    /// This surface as something the [`join`](crate::join::join) can weigh.
    ///
    /// The pid is unwrapped from the origin here rather than stored twice: an
    /// unattributed surface has no pid, and the join's own gate refuses to
    /// match on an absent one, so the two rules meet without either having to
    /// know about the other.
    ///
    /// A layer surface has no title, so its namespace stands in for one: a
    /// shell that names each accessibility window after its surface's
    /// namespace is then joined surface by surface, where one process with a
    /// wallpaper, a panel and a menu would otherwise be ambiguous. Like a
    /// title it only separates surfaces the pid has already admitted, so a
    /// namespace can never join a window to another process's surface.
    #[must_use]
    pub fn claim(&self) -> SurfaceClaim {
        let namespace = match &self.kind {
            SurfaceKind::Layer { namespace, .. } => Some(namespace.clone()),
            SurfaceKind::Window | SurfaceKind::LockCover => None,
        };
        SurfaceClaim {
            surface: self.id,
            title: self.title.clone().or(namespace),
            pid: match &self.origin {
                Origin::Process(process) => Some(process.pid),
                // The X client's pid, however it was learned: the join uses
                // it to pair a window with an accessibility tree, which is a
                // correlation it weighs, not an attestation it trusts.
                Origin::X11(x11) => x11.client.as_ref().map(|client| client.pid),
                Origin::Unattributed => None,
            },
            focused_at: self.focused_at,
        }
    }

    /// The same surface, not mapped.
    #[must_use]
    pub fn unmapped(mut self) -> Self {
        self.mapped = false;
        self
    }

    /// The same surface, unmapped because it is on workspace `number`, which
    /// is not showing.
    #[must_use]
    pub fn on_workspace(mut self, number: u16) -> Self {
        self.mapped = false;
        self.off_workspace = Some(number);
        self
    }

    /// The same surface, unmapped because it is a tab behind `shown` in its
    /// tab group.
    #[must_use]
    pub fn behind_tab(mut self, shown: SurfaceId) -> Self {
        self.mapped = false;
        self.behind_tab = Some(shown);
        self
    }

    /// The same surface, with a frame the host drew around it, in global
    /// space.
    #[must_use]
    pub fn framed(mut self, frame: impl IntoIterator<Item = Rect>) -> Self {
        self.frame = frame.into_iter().collect();
        self
    }

    /// The same surface, declaring where it is opaque in surface-local
    /// coordinates.
    #[must_use]
    pub fn declaring_opaque(mut self, regions: impl IntoIterator<Item = Rect>) -> Self {
        self.opaque = Some(regions.into_iter().collect());
        self
    }

    /// The same surface, attributed to an origin.
    #[must_use]
    pub fn owned_by(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// The same surface, a tab of a group of `tabs`, in tab order.
    #[must_use]
    pub fn tabbed(mut self, tabs: impl IntoIterator<Item = SurfaceId>) -> Self {
        self.tabs = tabs.into_iter().collect();
        self
    }

    /// The same surface, a layer-shell surface in `layer` with `namespace`.
    #[must_use]
    pub fn layered(mut self, layer: Layer, namespace: impl Into<String>) -> Self {
        self.kind = SurfaceKind::Layer {
            layer,
            namespace: namespace.into(),
        };
        self
    }

    /// The same surface, a screen locker's cover.
    #[must_use]
    pub fn lock_cover(mut self) -> Self {
        self.kind = SurfaceKind::LockCover;
        self
    }

    /// The same surface, with the title its client set.
    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The same surface, with surface-local `(0, 0)` somewhere other than the
    /// window geometry's own origin -- which is what client-side decoration
    /// does.
    #[must_use]
    pub fn with_buffer_origin(mut self, origin: Vec2) -> Self {
        self.buffer_origin = origin;
        self
    }

    /// The same surface, damaged `generation` times.
    #[must_use]
    pub fn damaged(mut self, generation: u64) -> Self {
        self.damage_generation = generation;
        self
    }

    /// The same surface, with a recorded history of where damage landed.
    #[must_use]
    pub fn damaging(mut self, history: impl IntoIterator<Item = (u64, Rect)>) -> Self {
        self.damage = history.into_iter().collect();
        self.damage_generation = self.damage.last().map_or(0, |(generation, _)| *generation);
        self
    }

    /// Everything that has changed on this surface since `reconciled`, in
    /// global space, or `None` if nothing has.
    ///
    /// Three answers, and the third is the one worth reading. Nothing newer
    /// than `reconciled` is `None`. Damage this history still holds is the
    /// union of those regions. And damage older than the history reaches is the
    /// **whole surface**: a reader that has fallen further behind than the
    /// compositor remembers cannot be told what changed, and the honest answer
    /// to "what did I miss" is "possibly all of it".
    #[must_use]
    pub fn damage_since(&self, reconciled: u64) -> Option<Rect> {
        if self.damage_generation <= reconciled {
            return None;
        }
        match self.damage.first() {
            // The first thing the history still holds is newer than the first
            // thing this reader has not seen, so something in between was
            // dropped.
            Some((oldest, _)) if *oldest > reconciled + 1 => Some(self.geometry),
            None => Some(self.geometry),
            Some(_) => self
                .damage
                .iter()
                .filter(|(generation, _)| *generation > reconciled)
                .map(|(_, region)| self.surface_local_to_global(*region))
                .reduce(|left, right| left.union(right)),
        }
    }

    /// A window-space rect, in global space.
    ///
    /// Window space, not node space: a node's bounds come here through
    /// [`Index::window_bounds`](crate::Index::window_bounds), which has
    /// already measured where its toolkit's origin is.
    #[must_use]
    pub fn to_global(&self, rect: Rect) -> Rect {
        rect + self.geometry.origin().to_vec2()
    }

    /// Whether damage newer than `reconciled` landed on a window-space rect.
    ///
    /// The region scoping is the whole point and it is what only a compositor
    /// can do. "This surface changed" is nearly always true -- an idle
    /// gtk4-widget-factory repaints its whole window about forty times a second
    /// -- and therefore says nothing about whether a particular button
    /// reacted. "Pixels changed *here*" is a different claim, and it is the one
    /// worth putting in a receipt.
    ///
    /// Still evidence rather than proof, in one direction: a toolkit that
    /// repaints wholesale damages every rect on it, including this one, whether
    /// or not the act did anything. A `false` is the strong answer -- nothing
    /// happened here at all -- and a `true` is only as informative as the
    /// surface's idle rate makes it.
    #[must_use]
    pub fn damage_touches(&self, reconciled: u64, rect: Rect) -> bool {
        self.damage_since(reconciled)
            .is_some_and(|region| overlaps(self.to_global(rect), region))
    }

    /// Whether this surface proves that `global` shows through it.
    ///
    /// Proof, not likelihood -- see the module's policy. A surface that
    /// declared nothing proves nothing.
    fn proves_transparent(&self, global: Rect) -> bool {
        let Some(regions) = &self.opaque else {
            return false;
        };
        let covered = global.intersect(self.geometry);
        !regions
            .iter()
            .any(|region| overlaps(self.surface_local_to_global(*region), covered))
    }

    /// A surface-local region, in global space. Regions are placed from
    /// `buffer_origin`, which is the protocol's own origin, and not from
    /// `geometry`'s, which under client-side decoration is the shadow's width
    /// inside it.
    fn surface_local_to_global(&self, region: Rect) -> Rect {
        let (dx, dy) = (self.buffer_origin.x, self.buffer_origin.y);
        Rect::new(
            region.x0 + dx,
            region.y0 + dy,
            region.x1 + dx,
            region.y1 + dy,
        )
    }
}

/// Everything a host knows about every surface, at one instant.
///
/// Z-order is carried as the order of the surfaces themselves, bottom to top,
/// because that is the only way to store it that cannot disagree with itself. A
/// `z: usize` field per surface can be duplicated or left with a gap by a
/// producer that reorders one entry and forgets another; a list cannot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostFacts {
    surfaces: Vec<SurfaceFacts>,
    generation: u64,
    consent: Consent,
    outputs: Vec<Rect>,
}

impl HostFacts {
    /// Facts about surfaces given **bottom to top**.
    ///
    /// `generation` is the host's own publication counter: it changes whenever
    /// anything here does, so a reader can tell one snapshot from another
    /// without comparing them.
    #[must_use]
    pub fn bottom_to_top(
        surfaces: impl IntoIterator<Item = SurfaceFacts>,
        generation: u64,
    ) -> Self {
        Self {
            surfaces: surfaces.into_iter().collect(),
            generation,
            consent: Consent::Nobody,
            outputs: Vec::new(),
        }
    }

    /// The same facts, with the monitors' rects in the global space. Without
    /// this, nothing is judged [`Visibility::OffScreen`]: a host that says
    /// nothing about its outputs is not claiming that a window is on none.
    #[must_use]
    pub fn with_outputs(mut self, outputs: impl IntoIterator<Item = Rect>) -> Self {
        self.outputs = outputs.into_iter().collect();
        self
    }

    /// Every output's rect in the global space, as the host published them.
    #[must_use]
    pub fn outputs(&self) -> &[Rect] {
        &self.outputs
    }

    /// The same facts, with the host's [`Consent`] policy. Without this,
    /// consent is [`Consent::Nobody`] and nothing is actable.
    #[must_use]
    pub fn with_consent(mut self, consent: Consent) -> Self {
        self.consent = consent;
        self
    }

    /// Whose applications an agent may act on, as the host published it.
    #[must_use]
    pub fn consent(&self) -> &Consent {
        &self.consent
    }

    /// The host's publication counter for this snapshot.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Every surface, bottom to top.
    #[must_use]
    pub fn surfaces(&self) -> &[SurfaceFacts] {
        &self.surfaces
    }

    /// The facts about one surface, or `None` if this host has never heard of
    /// it -- which is a real answer and not an error, because a node can
    /// outlive the surface it was read from.
    #[must_use]
    pub fn surface(&self, id: SurfaceId) -> Option<&SurfaceFacts> {
        self.surfaces.iter().find(|surface| surface.id == id)
    }
}

/// A visibility verdict, and the one thing worth knowing about how it was
/// reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judgement {
    /// What the node's visibility is.
    pub visibility: Visibility,
    /// Whether an `Occluded` verdict rests on policy rather than proof: the
    /// occluding surface overlapped the node and never declared an opaque
    /// region, so nothing established that the node shows through it.
    ///
    /// Counted rather than resolved. A high rate means real toolkits are not
    /// declaring opacity and the policy is refusing too much -- a finding that
    /// should arrive as a number in a log, not as a person reporting that
    /// everything is occluded.
    pub unproven: bool,
}

impl Judgement {
    /// A verdict that needed no policy to reach.
    fn proven(visibility: Visibility) -> Self {
        Self {
            visibility,
            unproven: false,
        }
    }
}

/// Whether a rect on a surface can actually be seen.
///
/// The order of the tests is the interesting part, because each one makes the
/// next meaningful:
///
/// 1. A surface this host has never heard of leaves the node `Unknown`, which
///    the gate refuses. Not `Unmapped` -- "I do not know" and "it is not on
///    screen" are different claims and only one of them is true.
/// 2. An unmapped surface is `Unmapped` before any arithmetic, because the
///    geometry of a surface that is not on screen means nothing -- or
///    `OtherWorkspace`, when it is unmapped only because the workspace it is
///    on is not the one showing, or `InactiveTab`, when it is a tab behind
///    another. A tab behind another in a group on a hidden workspace is
///    `OtherWorkspace`: the coarser reason, and the one to clear first.
/// 3. A zero-area rect is `Clipped`. Toolkits report `0x0` extents for widgets
///    they have realised but not laid out, and for children scrolled out of a
///    viewport; a zero-area rect is also the one input for which every
///    intersection test below would silently answer "nothing overlaps".
/// 4. A rect not wholly inside its own surface is `Clipped` -- the
///    application's own doing, and a different remedy from occlusion: scroll it
///    into view rather than raise anything.
/// 5. A rect not wholly on the monitors is `OffScreen`: its window hangs
///    past the edge of the desk, or was left on a monitor that is gone.
///    Before occlusion, because raising a window over it would still leave it
///    where nobody can see it.
/// 6. Only then, occlusion, from the top down, so the surface named is the one
///    an agent has to deal with first. A surface's frame is part of it: a
///    titlebar over the node occludes it, and proves it, because the host
///    drew the titlebar solid. A window's own frame never covers its own
///    nodes, because it is outside the window and test 4 has already said
///    `Clipped` of anything out there.
#[must_use]
pub fn judge(facts: &HostFacts, surface: SurfaceId, rect: Rect) -> Judgement {
    let Some(target) = facts.surface(surface) else {
        return Judgement::proven(Visibility::Unknown);
    };
    if !target.mapped {
        return Judgement::proven(match (target.off_workspace, target.behind_tab) {
            (Some(workspace), _) => Visibility::OtherWorkspace { workspace },
            (None, Some(shown)) => Visibility::InactiveTab { shown },
            (None, None) => Visibility::Unmapped,
        });
    }
    if rect.abs().is_empty() {
        return Judgement::proven(Visibility::Clipped);
    }

    let global = target.to_global(rect);
    if !contains(target.geometry, global) {
        return Judgement::proven(Visibility::Clipped);
    }
    if !facts.outputs.is_empty() && !on_outputs(&facts.outputs, global) {
        return Judgement::proven(Visibility::OffScreen);
    }

    // Top down: with two surfaces over one node, raising the topmost is what
    // makes it visible, so that is the one worth naming.
    let position = facts
        .surfaces()
        .iter()
        .position(|candidate| candidate.id == surface);
    let above = position.map_or(0, |index| index + 1);
    for candidate in facts.surfaces()[above..].iter().rev() {
        if !candidate.mapped {
            continue;
        }
        if candidate.frame.iter().any(|strip| overlaps(*strip, global)) {
            return Judgement::proven(Visibility::Occluded { by: candidate.id });
        }
        if !overlaps(candidate.geometry, global) {
            continue;
        }
        if candidate.proves_transparent(global) {
            continue;
        }
        return Judgement {
            visibility: Visibility::Occluded { by: candidate.id },
            unproven: candidate.opaque.is_none(),
        };
    }

    Judgement::proven(Visibility::Visible)
}

/// Whether two rects share any area. Touching edges do not count: a rect that
/// meets another exactly at its boundary covers none of it.
pub(crate) fn overlaps(a: Rect, b: Rect) -> bool {
    !a.intersect(b).is_empty()
}

/// Whether every point of `rect` is on some output. Measured as area, which
/// is exact for outputs that do not overlap, and only ever generous for ones
/// that do: a mirror counts its shared part twice.
fn on_outputs(outputs: &[Rect], rect: Rect) -> bool {
    let covered: f64 = outputs
        .iter()
        .map(|output| output.intersect(rect).area())
        .sum();
    covered >= rect.area()
}

/// Whether `inner` lies wholly within `outer`.
fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x0 >= outer.x0 && inner.y0 >= outer.y0 && inner.x1 <= outer.x1 && inner.y1 <= outer.y1
}

/// What a pass of judgement decided, in aggregate.
///
/// Exists so that a policy can be *watched*. Two ratios carry the information:
/// `unproven` against `occluded`, which is the share of refusals resting on a
/// client having declared nothing, and `unjudged`, which should be zero once a
/// host is joined. When it is not, either the join has a bug or a bridge left
/// a window with no extents to measure from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Nodes judged in this pass.
    pub judged: usize,
    /// On screen and unobstructed.
    pub visible: usize,
    /// Covered by another surface.
    pub occluded: usize,
    /// Of `occluded`, how many rest on policy rather than proof.
    pub unproven: usize,
    /// Clipped by their own window.
    pub clipped: usize,
    /// On a surface that is not mapped.
    pub unmapped: usize,
    /// Not wholly on any output.
    pub off_screen: usize,
    /// Not judged at all: no surface joined, no bounds reported, a window
    /// whose own node reported none to measure from, or a surface the host
    /// has never heard of.
    pub unjudged: usize,
}

impl Tally {
    /// Count one verdict.
    pub fn record(&mut self, judgement: &Judgement) {
        self.judged += 1;
        if judgement.unproven {
            self.unproven += 1;
        }
        match judgement.visibility {
            Visibility::Visible => self.visible += 1,
            Visibility::Occluded { .. } => self.occluded += 1,
            Visibility::Clipped => self.clipped += 1,
            Visibility::Unmapped
            | Visibility::OtherWorkspace { .. }
            | Visibility::InactiveTab { .. } => self.unmapped += 1,
            Visibility::OffScreen => self.off_screen += 1,
            Visibility::Unknown => self.unjudged += 1,
        }
    }
}

impl core::fmt::Display for Tally {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} judged: {} visible, {} occluded ({} unproven), {} clipped, \
             {} unmapped, {} off screen, {} unjudged",
            self.judged,
            self.visible,
            self.occluded,
            self.unproven,
            self.clipped,
            self.unmapped,
            self.off_screen,
            self.unjudged
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::{Finding, WindowClaim, join};
    use perspicax_node::{NodeId, ProcessOrigin};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    /// One window at the origin, 400x300, with a button in the middle of it.
    fn window() -> SurfaceFacts {
        SurfaceFacts::new(SurfaceId(1), rect(0.0, 0.0, 400.0, 300.0))
    }

    const BUTTON: Rect = Rect {
        x0: 100.0,
        y0: 100.0,
        x1: 200.0,
        y1: 140.0,
    };

    fn verdict(facts: &HostFacts) -> Judgement {
        judge(facts, SurfaceId(1), BUTTON)
    }

    #[test]
    fn an_unobstructed_node_is_visible() {
        let facts = HostFacts::bottom_to_top([window()], 1);
        assert_eq!(verdict(&facts), Judgement::proven(Visibility::Visible));
    }

    /// "I have never heard of that surface" and "it is not on screen" are
    /// different claims. Only the second is `Unmapped`, and reporting it for
    /// the first would be a compositor asserting something it does not know.
    #[test]
    fn an_unheard_of_surface_is_unknown_rather_than_unmapped() {
        let facts = HostFacts::bottom_to_top([window()], 1);
        assert_eq!(
            judge(&facts, SurfaceId(99), BUTTON).visibility,
            Visibility::Unknown
        );
    }

    #[test]
    fn a_window_on_another_workspace_says_which() {
        let facts = HostFacts::bottom_to_top([window().on_workspace(3)], 1);
        assert_eq!(
            verdict(&facts).visibility,
            Visibility::OtherWorkspace { workspace: 3 }
        );
    }

    #[test]
    fn a_tab_behind_another_names_the_one_in_front() {
        let facts = HostFacts::bottom_to_top([window().behind_tab(SurfaceId(7))], 1);
        assert_eq!(
            verdict(&facts),
            Judgement::proven(Visibility::InactiveTab {
                shown: SurfaceId(7)
            })
        );
    }

    /// Both at once: the group is on another workspace, which is the reason
    /// to clear first, so it is the one given.
    #[test]
    fn a_hidden_workspace_outranks_a_hidden_tab() {
        let facts =
            HostFacts::bottom_to_top([window().behind_tab(SurfaceId(7)).on_workspace(2)], 1);
        assert_eq!(
            verdict(&facts).visibility,
            Visibility::OtherWorkspace { workspace: 2 }
        );
    }

    #[test]
    fn nothing_on_an_unmapped_surface_can_be_seen() {
        let facts = HostFacts::bottom_to_top([window().unmapped()], 1);
        assert_eq!(verdict(&facts).visibility, Visibility::Unmapped);
    }

    /// A realised-but-unlaid-out widget, and a child scrolled out of a
    /// viewport, both arrive as `0x0` from a real toolkit.
    #[test]
    fn a_zero_area_rect_is_clipped() {
        let facts = HostFacts::bottom_to_top([window()], 1);
        assert_eq!(
            judge(&facts, SurfaceId(1), rect(50.0, 50.0, 50.0, 50.0)).visibility,
            Visibility::Clipped
        );
    }

    #[test]
    fn a_node_hanging_outside_its_own_window_is_clipped() {
        let facts = HostFacts::bottom_to_top([window()], 1);
        assert_eq!(
            judge(&facts, SurfaceId(1), rect(380.0, 10.0, 460.0, 50.0)).visibility,
            Visibility::Clipped
        );
    }

    /// The policy, in one test: a surface on top that declared nothing about
    /// its opacity is not assumed to be see-through, and the verdict says so.
    #[test]
    fn a_surface_above_that_declares_nothing_occludes_on_policy() {
        let cover = SurfaceFacts::new(SurfaceId(2), rect(150.0, 120.0, 300.0, 200.0));
        let facts = HostFacts::bottom_to_top([window(), cover], 1);
        assert_eq!(
            verdict(&facts),
            Judgement {
                visibility: Visibility::Occluded { by: SurfaceId(2) },
                unproven: true,
            }
        );
    }

    /// A titlebar is pixels no client drew. One over a node covers it, and
    /// proves it, even when the window it belongs to declared its own
    /// surface fully transparent: the frame is not the surface.
    #[test]
    fn a_titlebar_over_a_node_occludes_it_as_a_matter_of_proof() {
        let below = rect(0.0, 150.0, 400.0, 400.0);
        let titlebar = rect(0.0, 124.0, 400.0, 150.0);
        let cover = SurfaceFacts::new(SurfaceId(2), below)
            .declaring_opaque([])
            .framed([titlebar]);
        let facts = HostFacts::bottom_to_top([window(), cover], 1);
        assert_eq!(
            verdict(&facts),
            Judgement::proven(Visibility::Occluded { by: SurfaceId(2) })
        );

        let elsewhere = SurfaceFacts::new(SurfaceId(2), rect(500.0, 500.0, 600.0, 600.0))
            .framed([rect(500.0, 474.0, 600.0, 500.0)]);
        let facts = HostFacts::bottom_to_top([window(), elsewhere], 1);
        assert_eq!(verdict(&facts), Judgement::proven(Visibility::Visible));
    }

    /// A window's own titlebar is not something its nodes can be under.
    #[test]
    fn a_window_is_not_occluded_by_its_own_frame() {
        let framed = window().framed([rect(0.0, -26.0, 400.0, 0.0)]);
        let facts = HostFacts::bottom_to_top([framed], 1);
        assert_eq!(verdict(&facts), Judgement::proven(Visibility::Visible));
    }

    /// And the other half: a client that *did* declare where it is opaque is
    /// believed, both when the node falls inside that region and when it does
    /// not. Neither verdict is `unproven`, because both rest on something the
    /// client said rather than on our default.
    #[test]
    fn a_declared_opaque_region_is_believed_in_both_directions() {
        let over = rect(150.0, 120.0, 300.0, 200.0);

        let opaque_there =
            SurfaceFacts::new(SurfaceId(2), over).declaring_opaque([rect(0.0, 0.0, 150.0, 80.0)]);
        let facts = HostFacts::bottom_to_top([window(), opaque_there], 1);
        assert_eq!(
            verdict(&facts),
            Judgement {
                visibility: Visibility::Occluded { by: SurfaceId(2) },
                unproven: false,
            }
        );

        // Opaque only in its own bottom-right corner, which is nowhere near
        // the button. The button shows through.
        let opaque_elsewhere = SurfaceFacts::new(SurfaceId(2), over)
            .declaring_opaque([rect(100.0, 60.0, 150.0, 80.0)]);
        let facts = HostFacts::bottom_to_top([window(), opaque_elsewhere], 1);
        assert_eq!(verdict(&facts), Judgement::proven(Visibility::Visible));
    }

    /// `None` and `Some(vec![])` are different claims, which is the reason the
    /// field is an `Option`: nothing declared, versus declared entirely
    /// transparent.
    #[test]
    fn declaring_no_opaque_region_at_all_is_a_claim_of_transparency() {
        let ghost =
            SurfaceFacts::new(SurfaceId(2), rect(150.0, 120.0, 300.0, 200.0)).declaring_opaque([]);
        let facts = HostFacts::bottom_to_top([window(), ghost], 1);
        assert_eq!(verdict(&facts), Judgement::proven(Visibility::Visible));
    }

    #[test]
    fn a_node_past_the_edge_of_the_monitors_is_off_screen() {
        // The window hangs off the right of a 150-wide output.
        let facts =
            HostFacts::bottom_to_top([window()], 1).with_outputs([rect(0.0, 0.0, 150.0, 300.0)]);
        assert_eq!(verdict(&facts).visibility, Visibility::OffScreen);
    }

    #[test]
    fn a_node_across_two_monitors_is_on_screen() {
        let facts = HostFacts::bottom_to_top([window()], 1)
            .with_outputs([rect(0.0, 0.0, 150.0, 300.0), rect(150.0, 0.0, 400.0, 300.0)]);
        assert_eq!(verdict(&facts).visibility, Visibility::Visible);
    }

    #[test]
    fn a_node_in_the_dead_strip_below_a_shorter_monitor_is_off_screen() {
        // The right monitor stops at y = 120; the button reaches 140.
        let facts = HostFacts::bottom_to_top([window()], 1)
            .with_outputs([rect(0.0, 0.0, 150.0, 300.0), rect(150.0, 0.0, 400.0, 120.0)]);
        assert_eq!(verdict(&facts).visibility, Visibility::OffScreen);
    }

    #[test]
    fn a_surface_below_does_not_occlude() {
        let under = SurfaceFacts::new(SurfaceId(0), rect(150.0, 120.0, 300.0, 200.0));
        let facts = HostFacts::bottom_to_top([under, window()], 1);
        assert_eq!(verdict(&facts).visibility, Visibility::Visible);
    }

    #[test]
    fn an_unmapped_surface_above_does_not_occlude() {
        let closed_menu =
            SurfaceFacts::new(SurfaceId(2), rect(150.0, 120.0, 300.0, 200.0)).unmapped();
        let facts = HostFacts::bottom_to_top([window(), closed_menu], 1);
        assert_eq!(verdict(&facts).visibility, Visibility::Visible);
    }

    /// With two windows over one node, raising the lower one changes nothing.
    /// Naming the topmost is what lets an agent recover in one step.
    #[test]
    fn the_topmost_occluder_is_the_one_named() {
        let middle = SurfaceFacts::new(SurfaceId(2), rect(150.0, 120.0, 300.0, 200.0));
        let top = SurfaceFacts::new(SurfaceId(3), rect(90.0, 90.0, 400.0, 300.0));
        let facts = HostFacts::bottom_to_top([window(), middle, top], 1);
        assert_eq!(
            verdict(&facts).visibility,
            Visibility::Occluded { by: SurfaceId(3) }
        );
    }

    #[test]
    fn touching_edges_do_not_occlude() {
        let flush = SurfaceFacts::new(SurfaceId(2), rect(200.0, 100.0, 300.0, 140.0));
        let facts = HostFacts::bottom_to_top([window(), flush], 1);
        assert_eq!(verdict(&facts).visibility, Visibility::Visible);
    }

    /// A window that is not at the origin is where a coordinate mistake would
    /// actually show up, so the arithmetic gets its own test rather than being
    /// implied by the ones above.
    #[test]
    fn occlusion_is_decided_in_global_space() {
        let moved = SurfaceFacts::new(SurfaceId(1), rect(500.0, 400.0, 900.0, 700.0));
        // Overlaps the button's *global* position (600,500)-(700,540), and not
        // its window-relative one.
        let cover = SurfaceFacts::new(SurfaceId(2), rect(650.0, 450.0, 800.0, 600.0));
        let facts = HostFacts::bottom_to_top([moved, cover], 1);
        assert_eq!(
            verdict(&facts).visibility,
            Visibility::Occluded { by: SurfaceId(2) }
        );

        let elsewhere = SurfaceFacts::new(SurfaceId(2), rect(100.0, 100.0, 200.0, 140.0));
        let facts = HostFacts::bottom_to_top(
            [
                SurfaceFacts::new(SurfaceId(1), rect(500.0, 400.0, 900.0, 700.0)),
                elsewhere,
            ],
            1,
        );
        assert_eq!(verdict(&facts).visibility, Visibility::Visible);
    }

    /// Under client-side decoration the buffer starts outside the visible
    /// frame, so a surface's two origins are not the same point. An opaque
    /// region declared at surface-local `(0, 0)` lands at the outside of the
    /// shadow, not at the corner of the window.
    #[test]
    fn a_decorated_surface_has_two_origins_and_they_are_used_for_different_things() {
        let decorated = SurfaceFacts::new(SurfaceId(1), rect(100.0, 100.0, 500.0, 400.0))
            .with_buffer_origin(Vec2::new(80.0, 80.0));
        assert_eq!(
            decorated.surface_local_to_global(rect(0.0, 0.0, 10.0, 10.0)),
            rect(80.0, 80.0, 90.0, 90.0),
            "a region is placed from the buffer's origin"
        );
        assert_eq!(
            decorated.to_global(rect(0.0, 0.0, 10.0, 10.0)),
            rect(100.0, 100.0, 110.0, 110.0),
            "a window-space rect is placed from the window geometry's origin"
        );
    }

    #[test]
    fn facts_carry_their_producers_generation_and_origins() {
        let origin = Origin::Process(Box::new(ProcessOrigin {
            pid: 9182,
            exe: Some("/usr/bin/gtk4-widget-factory".into()),
            cgroup: None,
            sandbox: None,
        }));
        let facts = HostFacts::bottom_to_top([window().owned_by(origin.clone()).damaged(3)], 42);
        let surface = facts.surface(SurfaceId(1)).unwrap();
        assert_eq!(facts.generation(), 42);
        assert_eq!(surface.origin, origin);
        assert_eq!(surface.damage_generation, 3);
        assert!(facts.surface(SurfaceId(2)).is_none());
    }

    /// The three answers `damage_since` gives, and the third is the one that
    /// keeps a reader honest rather than merely informed.
    #[test]
    fn damage_since_says_nothing_something_or_everything() {
        let surface = window().damaging([
            (1, rect(0.0, 0.0, 10.0, 10.0)),
            (2, rect(300.0, 200.0, 320.0, 220.0)),
            (3, rect(310.0, 210.0, 330.0, 230.0)),
        ]);

        assert_eq!(surface.damage_since(3), None, "nothing newer than the read");
        assert_eq!(
            surface.damage_since(1),
            Some(rect(300.0, 200.0, 330.0, 230.0)),
            "the union of what is newer, and not a whole-window panic"
        );

        // A reader further behind than the history reaches cannot be told what
        // it missed, so it is told it missed everything.
        let truncated = window().damaging([(9, rect(0.0, 0.0, 10.0, 10.0))]);
        assert_eq!(
            truncated.damage_since(3),
            Some(truncated.geometry),
            "a gap in the history is answered with the whole surface"
        );
        assert_eq!(
            truncated.damage_since(8),
            Some(rect(0.0, 0.0, 10.0, 10.0)),
            "and an unbroken history is not"
        );
    }

    /// Damage arrives in surface-local coordinates, so under decoration it is
    /// placed from the buffer's origin and not the window geometry's.
    #[test]
    fn damage_is_placed_from_the_buffer_origin() {
        let decorated = SurfaceFacts::new(SurfaceId(1), rect(100.0, 100.0, 500.0, 400.0))
            .with_buffer_origin(Vec2::new(80.0, 80.0))
            .damaging([(1, rect(0.0, 0.0, 10.0, 10.0))]);
        assert_eq!(
            decorated.damage_since(0),
            Some(rect(80.0, 80.0, 90.0, 90.0))
        );
    }

    #[test]
    fn a_tally_counts_unproven_within_occluded_not_beside_it() {
        let mut tally = Tally::default();
        tally.record(&Judgement::proven(Visibility::Visible));
        tally.record(&Judgement {
            visibility: Visibility::Occluded { by: SurfaceId(2) },
            unproven: true,
        });
        tally.record(&Judgement {
            visibility: Visibility::Occluded { by: SurfaceId(3) },
            unproven: false,
        });
        tally.record(&Judgement::proven(Visibility::Unknown));

        assert_eq!(tally.judged, 4);
        assert_eq!(tally.occluded, 2);
        assert_eq!(tally.unproven, 1);
        assert_eq!(tally.unjudged, 1);
        assert_eq!(
            tally.to_string(),
            "4 judged: 1 visible, 2 occluded (1 unproven), 0 clipped, \
             0 unmapped, 0 off screen, 1 unjudged"
        );
    }

    fn process(pid: u32) -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid,
            exe: Some("/usr/bin/perspicax-shell".into()),
            cgroup: None,
            sandbox: None,
        }))
    }

    fn named(node: u64, title: &str, pid: u32) -> WindowClaim {
        WindowClaim {
            node: NodeId(node),
            title: Some(title.to_owned()),
            bus_pid: Some(pid),
            active_at: None,
        }
    }

    #[test]
    fn a_layer_surface_is_claimed_by_its_namespace() {
        let panel = window()
            .layered(Layer::Top, "perspicax-panel-DP-1")
            .owned_by(process(7));
        assert_eq!(panel.claim().title.as_deref(), Some("perspicax-panel-DP-1"));
        assert_eq!(window().lock_cover().claim().title, None);
        assert_eq!(
            window().titled("Files").claim().title.as_deref(),
            Some("Files")
        );
    }

    #[test]
    fn two_layer_surfaces_of_one_process_are_told_apart_by_namespace() {
        let wallpaper = SurfaceFacts::new(SurfaceId(1), rect(0.0, 0.0, 1920.0, 1080.0))
            .layered(Layer::Background, "perspicax-desktop-DP-1")
            .owned_by(process(7));
        let panel = SurfaceFacts::new(SurfaceId(2), rect(0.0, 1040.0, 1920.0, 1080.0))
            .layered(Layer::Top, "perspicax-panel-DP-1")
            .owned_by(process(7));
        let (joins, findings) = join(
            &[
                named(10, "perspicax-panel-DP-1", 7),
                named(11, "perspicax-desktop-DP-1", 7),
            ],
            &[wallpaper.claim(), panel.claim()],
        );
        assert!(findings.is_empty(), "{findings:?}");
        let surface_of = |node| {
            joins
                .iter()
                .find(|join| join.node == NodeId(node))
                .map(|join| join.surface)
        };
        assert_eq!(surface_of(10), Some(SurfaceId(2)));
        assert_eq!(surface_of(11), Some(SurfaceId(1)));
    }

    #[test]
    fn a_namespace_never_joins_across_processes() {
        let panel = window()
            .layered(Layer::Top, "perspicax-panel-DP-1")
            .owned_by(process(7));
        let (joins, findings) = join(&[named(10, "perspicax-panel-DP-1", 8)], &[panel.claim()]);
        assert!(joins.is_empty());
        assert!(
            matches!(findings[..], [Finding::Contradiction { .. }]),
            "{findings:?}"
        );
    }

    #[test]
    fn layers_are_named_as_layer_shell_spells_them() {
        let names: Vec<_> = [Layer::Background, Layer::Bottom, Layer::Top, Layer::Overlay]
            .into_iter()
            .map(Layer::name)
            .collect();
        assert_eq!(names, ["background", "bottom", "top", "overlay"]);
    }
}
