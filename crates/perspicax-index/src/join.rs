//! Binding what a bridge describes to what a host drew.
//!
//! An accessibility bridge offers a tree of windows. A compositor holds a list
//! of surfaces. Nothing in either connects them: the bridge does not know what
//! a surface is, and the compositor cannot read a widget tree. Everything this
//! project claims -- provenance, occlusion, a refusal that names the window in
//! the way -- rests on a correspondence that neither side reports and both
//! sides only supply evidence for.
//!
//! # Why this is a correlation and not a lookup
//!
//! The obvious answer is the pid, and it is necessary without being sufficient.
//! The compositor's pid comes from the credentials of a Wayland socket; the
//! bridge's comes from `GetConnectionUnixProcessID` on the accessibility bus.
//! Both are kernel-attested, and they attest to *different sockets*: one says
//! which process drew, the other says which process is serving accessibility.
//! Usually the same program. Not always -- AT-SPI's `Socket` interface exists
//! precisely so one process can serve trees it did not draw -- and one program
//! with four windows produces one pid and four surfaces, which a pid cannot
//! separate at all.
//!
//! So the join weighs evidence, and the rule when the evidence does not settle
//! it is the rule this whole project runs on: **a disagreement is a finding,
//! not a tie-break.** An unjoined window stays [`Origin::Unattributed`] and
//! every node beneath it is refused, which is a recoverable state that says so.
//! A guessed join is an agent clicking in the wrong application's window while
//! being told the receipt was fine.
//!
//! # Windows are structural, not a role
//!
//! Callers build [`WindowClaim`]s from the *children of an application's root*,
//! not by looking for [`Role::Window`](perspicax_node::Role::Window). GTK's
//! toplevel is an AT-SPI `frame` and maps to `Role::Window`; the Qt widget
//! gallery's is a `dialog` and maps to `Role::Dialog`. A role-shaped search
//! finds one toolkit, misses the other entirely, and reports an empty desktop
//! rather than an error.
//!
//! [`Origin::Unattributed`]: perspicax_node::Origin::Unattributed

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use perspicax_node::{NodeId, Rect, SurfaceId, Vec2};

/// How far apart a host's focus and a bridge's activation may be and still
/// describe the same event.
///
/// They are two observations of one thing travelling by different routes: the
/// compositor focuses a toplevel, the toolkit notices, and its bridge emits
/// `state-changed:active` over D-Bus. A quarter of a second is generous for
/// that path and far shorter than the interval between two deliberate window
/// switches, which is the only thing this has to avoid confusing.
pub const FOCUS_CORRELATION: Duration = Duration::from_millis(250);

/// A toplevel, as an accessibility bridge describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowClaim {
    /// The node the bridge offered as a child of its application root.
    pub node: NodeId,
    /// The window's name, which for both toolkits is its title.
    pub title: Option<String>,
    /// The pid the accessibility bus daemon observed for the connection
    /// serving this window. Attested, and about a bus connection rather than
    /// about whoever drew anything.
    pub bus_pid: Option<u32>,
    /// When the bridge last reported this window active.
    pub active_at: Option<Instant>,
}

/// A surface, as its host knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceClaim {
    /// The host's handle for it.
    pub surface: SurfaceId,
    /// The title the client set on its toplevel.
    pub title: Option<String>,
    /// The pid from the client's own connection credentials.
    pub pid: Option<u32>,
    /// When the host last gave this surface keyboard focus.
    pub focused_at: Option<Instant>,
}

/// One reason to believe a window and a surface are the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Two sockets, attested separately, name one process.
    Pid(u32),
    /// The title the client set and the name the bridge reports are the same
    /// string.
    Title,
    /// The host focused this surface and the bridge reported this window
    /// active, close enough together to be one event.
    Focus {
        /// How far apart the two observations were.
        skew: Duration,
    },
}

/// A window bound to a surface, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// The bridge's window node.
    pub node: NodeId,
    /// The host's surface.
    pub surface: SurfaceId,
    /// Everything that pointed this way, strongest first.
    pub evidence: Vec<Evidence>,
}

/// A window that could not be bound, and what stopped it.
///
/// Findings are the output that matters most when something is wrong, so they
/// are returned rather than logged: a caller can print them, count them, or
/// assert on them, and none of those work on a `tracing::warn!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// Several surfaces fit and nothing separates them.
    Ambiguous {
        /// The window that could not be placed.
        node: NodeId,
        /// Everything it might have been.
        candidates: Vec<SurfaceId>,
    },
    /// One axis of evidence pointed at a surface and another ruled it out --
    /// the titles matched and the processes did not. Worth surfacing rather
    /// than discarding, because it is what a bridge serving another process's
    /// tree looks like from here.
    Contradiction {
        /// The window that could not be placed.
        node: NodeId,
        /// The surface it agreed with on one axis.
        surface: SurfaceId,
        /// The process the accessibility bus named.
        bridge_pid: u32,
        /// The process the host named.
        host_pid: u32,
    },
    /// Nothing on the host could be this window.
    Unmatched {
        /// The window that could not be placed.
        node: NodeId,
    },
    /// Two windows both resolved to one surface, so at most one is right and
    /// this join cannot say which. Reported for every claimant.
    Contested {
        /// The window that could not be placed.
        node: NodeId,
        /// The surface more than one window claimed.
        surface: SurfaceId,
    },
}

impl Finding {
    /// The window this finding is about.
    #[must_use]
    pub fn node(&self) -> NodeId {
        match self {
            Self::Ambiguous { node, .. }
            | Self::Contradiction { node, .. }
            | Self::Unmatched { node }
            | Self::Contested { node, .. } => *node,
        }
    }
}

impl core::fmt::Display for Finding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ambiguous { node, candidates } => write!(
                f,
                "window {} matches {} surfaces equally well",
                node.0,
                candidates.len()
            ),
            Self::Contradiction {
                node,
                surface,
                bridge_pid,
                host_pid,
            } => write!(
                f,
                "window {} shares a title with surface {} but the bridge names \
                 process {bridge_pid} where the host names {host_pid}",
                node.0, surface.0
            ),
            Self::Unmatched { node } => {
                write!(f, "window {} matches no surface this host drew", node.0)
            }
            Self::Contested { node, surface } => write!(
                f,
                "window {} and another both resolve to surface {}",
                node.0, surface.0
            ),
        }
    }
}

/// Bind every window that can be bound, and say why the rest could not.
///
/// The process is the gate: a surface is a candidate only when both sides know
/// a pid and the two agree. Everything after that separates candidates rather
/// than admitting new ones, which is what keeps a title -- a string an
/// application chooses for itself and can change to anything -- from ever being
/// sufficient on its own.
#[must_use]
pub fn join(windows: &[WindowClaim], surfaces: &[SurfaceClaim]) -> (Vec<Join>, Vec<Finding>) {
    let mut joins = Vec::new();
    let mut findings = Vec::new();

    for window in windows {
        match bind(window, surfaces) {
            Ok(join) => joins.push(join),
            Err(finding) => findings.push(finding),
        }
    }

    // A surface can have drawn only one window. Two claimants means at least
    // one is wrong, and nothing here knows which -- so both are demoted, which
    // costs a refusal and never spends a wrong attribution.
    let mut claimants: HashMap<SurfaceId, usize> = HashMap::new();
    for join in &joins {
        *claimants.entry(join.surface).or_default() += 1;
    }
    let (kept, contested): (Vec<Join>, Vec<Join>) = joins
        .into_iter()
        .partition(|join| claimants.get(&join.surface) == Some(&1));
    findings.extend(contested.into_iter().map(|join| Finding::Contested {
        node: join.node,
        surface: join.surface,
    }));

    (kept, findings)
}

/// Bind one window, or say what stopped it.
fn bind(window: &WindowClaim, surfaces: &[SurfaceClaim]) -> Result<Join, Finding> {
    let candidates: Vec<&SurfaceClaim> = surfaces
        .iter()
        .filter(|surface| agree(window.bus_pid, surface.pid))
        .collect();

    let Some(pid) = window.bus_pid.filter(|_| !candidates.is_empty()) else {
        // Nothing shares this window's process. If something nonetheless shares
        // its title, that is a finding rather than an absence -- it is what a
        // bridge serving a tree it did not draw looks like from here.
        let impostor = surfaces.iter().find(|surface| contradicts(window, surface));
        return Err(match impostor {
            // Both pids are known here -- `contradicts` requires it -- so the
            // finding can name them. A refusal that says "the processes differ"
            // without saying which is a refusal nobody can act on, which is the
            // failure mode this project keeps arguing against.
            Some(surface) => Finding::Contradiction {
                node: window.node,
                surface: surface.surface,
                bridge_pid: window.bus_pid.unwrap_or_default(),
                host_pid: surface.pid.unwrap_or_default(),
            },
            None => Finding::Unmatched { node: window.node },
        });
    };

    let chosen = if let [only] = candidates[..] {
        only
    } else {
        // One process, several windows: the pid cannot separate them, which is
        // the case the other two axes exist for.
        let by_title: Vec<&&SurfaceClaim> = candidates
            .iter()
            .filter(|surface| titles_match(window, surface))
            .collect();
        match by_title[..] {
            [only] => only,
            _ => closest_focus(window, &candidates).ok_or(Finding::Ambiguous {
                node: window.node,
                candidates: candidates.iter().map(|s| s.surface).collect(),
            })?,
        }
    };

    let mut evidence = vec![Evidence::Pid(pid)];
    if titles_match(window, chosen) {
        evidence.push(Evidence::Title);
    }
    if let Some(skew) = skew(window, chosen) {
        evidence.push(Evidence::Focus { skew });
    }

    Ok(Join {
        node: window.node,
        surface: chosen.surface,
        evidence,
    })
}

/// Whether two optional pids are both known and equal. Unknown never agrees
/// with anything, including another unknown: this is the gate, and a gate that
/// opens on absence is not one.
fn agree(window: Option<u32>, surface: Option<u32>) -> bool {
    matches!((window, surface), (Some(a), Some(b)) if a == b)
}

/// Whether the two sides name one window the same thing while naming two
/// different processes.
///
/// Both pids have to be *known* and different. Two unknowns sharing a title
/// contradict nothing -- they are an absence of evidence, and reporting that as
/// a conflict would send somebody looking for a bridge impersonating an
/// application when the real answer is that nobody asked the bus for a pid.
fn contradicts(window: &WindowClaim, surface: &SurfaceClaim) -> bool {
    titles_match(window, surface)
        && matches!((window.bus_pid, surface.pid), (Some(a), Some(b)) if a != b)
}

/// Whether both sides name this window the same thing. An absent title on
/// either side is not a match.
fn titles_match(window: &WindowClaim, surface: &SurfaceClaim) -> bool {
    matches!((&window.title, &surface.title), (Some(a), Some(b)) if a == b)
}

/// How far apart the host's focus and the bridge's activation were, if both
/// happened and they are close enough to be one event.
fn skew(window: &WindowClaim, surface: &SurfaceClaim) -> Option<Duration> {
    let (active, focused) = (window.active_at?, surface.focused_at?);
    let skew = if active > focused {
        active - focused
    } else {
        focused - active
    };
    (skew <= FOCUS_CORRELATION).then_some(skew)
}

/// The candidate whose focus best explains this window's activation, when
/// exactly one does.
fn closest_focus<'a>(
    window: &WindowClaim,
    candidates: &'a [&'a SurfaceClaim],
) -> Option<&'a SurfaceClaim> {
    let mut correlated: Vec<(Duration, &SurfaceClaim)> = candidates
        .iter()
        .filter_map(|surface| skew(window, surface).map(|skew| (skew, *surface)))
        .collect();
    correlated.sort_by_key(|(skew, _)| *skew);

    match correlated[..] {
        [(_, only)] => Some(only),
        // A tie is not a winner. Two surfaces focused within microseconds of
        // one activation is exactly the situation where picking the nearer one
        // would be a coin toss wearing a measurement's clothes.
        [(first, surface), (second, _), ..] if first != second => Some(surface),
        _ => None,
    }
}

// # Menus
//
// A menu is a surface of its own (an `xdg_popup`), and its accessible nodes
// hang somewhere in its application's tree with nothing that names the
// surface. Measured on 2026-10-14 (issue #112), two toolkits put them in two
// places, measured from two origins:
//
// - GTK 3 measures a menu's nodes from the menu's own surface. They hang under
//   the combo box or menu bar item that opened it, or as an untitled window
//   of their own beside the application's windows.
// - GTK 4 measures them in the node space of the window they hang under,
//   like the rest of that window's nodes.
//
// No node is reliably the menu's size, either: GTK 3's menu-bar menus have
// no node of their own, GTK 4's measures the menu's whole picture, shadow
// included, or neither that nor its window. What does tell them apart is the
// frame. Read in the right one, a menu's extent lands on its popup. Read in
// the other, it lands somewhere else entirely, so a menu is joined to a popup
// only when exactly one frame puts it there.

/// The coordinates a menu's nodes are measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// The popup's own surface, as GTK 3 measures them.
    Own,
    /// The node space of the window the menu hangs under, as GTK 4 measures
    /// them.
    Window(SurfaceId),
}

/// An open popup, as its host placed it: what a menu's nodes must land on.
#[derive(Debug, Clone, PartialEq)]
pub struct PopupClaim {
    /// The popup's surface.
    pub surface: SurfaceId,
    /// The window or panel it hangs from, or `None` for a popup of nothing.
    pub from: Option<SurfaceId>,
    /// The process that drew it, from its client's credentials.
    pub pid: Option<u32>,
    /// When it was put on screen. A menu read before then may be one that
    /// has since closed.
    pub mapped_at: Option<Instant>,
    /// Where its surface-local `(0, 0)` is, in global space.
    pub buffer_origin: Vec2,
    /// Its window geometry, in global space.
    pub geometry: Rect,
    /// Everything it draws, its shadow included, in global space.
    pub drawn: Rect,
}

/// The window a menu's nodes hang under, as far as reading them in its
/// frame needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Under {
    /// The window's surface.
    pub surface: SurfaceId,
    /// Where its window geometry begins in its node space:
    /// [`Index::window_origin`](crate::Index::window_origin).
    pub origin: Vec2,
    /// Its window geometry, in global space.
    pub geometry: Rect,
}

/// Part of an application's tree that could be an open menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuClaim {
    /// The nodes that would be joined to the popup with all beneath them:
    /// the menu's own node, or, for a menu-bar item whose items are measured
    /// from the menu it opens, those items.
    pub roots: Vec<NodeId>,
    /// Where `roots` reach, together, in node space.
    pub extent: Rect,
    /// The process serving them, from the accessibility bus.
    pub pid: Option<u32>,
    /// The window they hang under, or `None` for a menu that is an
    /// application's window of its own.
    pub under: Option<Under>,
    /// When the least recently read of `roots` was read.
    pub seen_at: Option<Instant>,
    /// The claim to make instead when this one lands on no popup: the menu's
    /// items, for a menu whose own node is not where they are. GTK 3 measures
    /// a menu-bar item on the bar and the items it opens from their menu, so
    /// only the items land on it.
    pub items: Option<Box<MenuClaim>>,
}

/// A menu bound to the popup it is, and where that popup begins in the
/// menu's coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct PopupJoin {
    /// The popup's surface.
    pub popup: SurfaceId,
    /// What is joined to it.
    pub roots: Vec<NodeId>,
    /// The coordinates its nodes are measured in.
    pub frame: Frame,
    /// Where the popup's window geometry begins in that frame's node space:
    /// what its nodes' bounds are measured from, as a window's are from its
    /// window node.
    pub origin: Vec2,
}

/// An open popup no menu could be bound to, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuFinding {
    /// No menu of its application, read since it opened, lands on it.
    Unmatched {
        /// The popup.
        popup: SurfaceId,
    },
    /// Several do, or one that lands on it also lands on another popup, and
    /// nothing here says which is which. Joining none costs a refusal;
    /// joining the wrong one would put another menu's items where this one's
    /// are.
    Ambiguous {
        /// The popup.
        popup: SurfaceId,
        /// How many menus it could be.
        menus: usize,
    },
}

impl core::fmt::Display for MenuFinding {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unmatched { popup } => write!(
                f,
                "open menu {} matches none of its application's menus read since it opened",
                popup.0
            ),
            Self::Ambiguous { popup, menus } => write!(
                f,
                "open menu {} could be any of {menus} of its application's menus",
                popup.0
            ),
        }
    }
}

/// How much of a popup's window geometry a menu has to cover to be it, in
/// either frame. GTK 4's drop-down list covers 99% of its own, measuring its
/// shadow and not quite all of its window; anything well short is a part of
/// some menu, or a node that only happens to sit there.
const COVERS: f64 = 0.9;

/// Where `extent`, measured in `frame`, is in global space, or `None` for a
/// window frame on a menu that hangs under no window.
fn placed(popup: &PopupClaim, menu: &MenuClaim, frame: Frame) -> Option<Rect> {
    match (frame, menu.under) {
        (Frame::Own, _) => Some(menu.extent + popup.buffer_origin),
        (Frame::Window(_), Some(under)) => {
            Some(menu.extent - under.origin + under.geometry.origin().to_vec2())
        }
        (Frame::Window(_), None) => None,
    }
}

/// Whether `menu`, read in `frame`, is `popup`: inside what the popup draws,
/// a pixel of rounding allowed, and covering nearly all of its window.
#[must_use]
pub fn lands_on(popup: &PopupClaim, menu: &MenuClaim, frame: Frame) -> bool {
    let Some(global) = placed(popup, menu, frame) else {
        return false;
    };
    let drawn = popup.drawn;
    let inside = global.x0 >= drawn.x0 - 1.0
        && global.y0 >= drawn.y0 - 1.0
        && global.x1 <= drawn.x1 + 1.0
        && global.y1 <= drawn.y1 + 1.0;
    let geometry = popup.geometry.area();
    inside && geometry > 0.0 && global.intersect(popup.geometry).area() >= COVERS * geometry
}

/// Where `popup`'s window geometry begins in `frame`'s node space.
#[must_use]
pub fn popup_origin(popup: &PopupClaim, under: Option<&Under>, frame: Frame) -> Option<Vec2> {
    match (frame, under) {
        (Frame::Own, _) => Some(popup.geometry.origin() - popup.buffer_origin.to_point()),
        (Frame::Window(_), Some(under)) => {
            Some(under.origin + (popup.geometry.origin() - under.geometry.origin()))
        }
        (Frame::Window(_), None) => None,
    }
}

/// The one frame `menu` lands on `popup` in, if exactly one does.
fn the_frame(popup: &PopupClaim, menu: &MenuClaim) -> Option<Frame> {
    let mut frames = vec![Frame::Own];
    if let Some(under) = menu.under {
        frames.push(Frame::Window(under.surface));
    }
    match frames
        .into_iter()
        .filter(|frame| lands_on(popup, menu, *frame))
        .collect::<Vec<_>>()[..]
    {
        [only] => Some(only),
        _ => None,
    }
}

/// Whether `menu` could be `popup` at all, before any geometry: the same
/// process, read since the popup opened, and, for a menu under a window,
/// hung from that window.
fn eligible(popup: &PopupClaim, menu: &MenuClaim) -> bool {
    let fresh = matches!(
        (menu.seen_at, popup.mapped_at),
        (Some(seen), Some(mapped)) if seen >= mapped
    );
    let from = menu
        .under
        .is_none_or(|under| Some(under.surface) == popup.from);
    fresh && from && matches!((menu.pid, popup.pid), (Some(a), Some(b)) if a == b)
}

/// Bind each open popup to the one menu that is it, and say why the rest
/// could not be.
///
/// The rules are the window join's, applied to menus. A menu is a
/// candidate only from the same process, read since the popup opened, and
/// landing on it in exactly one frame. And a pairing stands only when it is
/// one to one: a popup two menus land on, or a menu that lands on two
/// popups, binds nothing, and every party to it is refused rather than
/// guessed. `popups` are taken in the order given, bottom to top, which is
/// the order their joins must be applied in, so that a submenu's nodes,
/// which hang beneath its menu's, end up on the submenu.
#[must_use]
pub fn join_popups(
    popups: &[PopupClaim],
    menus: &[MenuClaim],
) -> (Vec<PopupJoin>, Vec<MenuFinding>) {
    let lands_anywhere = |menu: &MenuClaim| {
        popups
            .iter()
            .any(|popup| eligible(popup, menu) && the_frame(popup, menu).is_some())
    };
    let menus: Vec<&MenuClaim> = menus
        .iter()
        .map(|menu| match &menu.items {
            Some(items) if !lands_anywhere(menu) => &**items,
            _ => menu,
        })
        .collect();
    let landed: Vec<Vec<(usize, Frame)>> = popups
        .iter()
        .map(|popup| {
            menus
                .iter()
                .enumerate()
                .filter(|(_, menu)| eligible(popup, menu))
                .filter_map(|(index, menu)| the_frame(popup, menu).map(|frame| (index, frame)))
                .collect()
        })
        .collect();
    let mut popups_per_menu: HashMap<usize, usize> = HashMap::new();
    for (index, _) in landed.iter().flatten() {
        *popups_per_menu.entry(*index).or_default() += 1;
    }

    let mut joins = Vec::new();
    let mut findings = Vec::new();
    for (popup, landed) in popups.iter().zip(landed) {
        match landed[..] {
            [] => findings.push(MenuFinding::Unmatched {
                popup: popup.surface,
            }),
            [(index, frame)] if popups_per_menu.get(&index) == Some(&1) => {
                let menu = menus[index];
                match popup_origin(popup, menu.under.as_ref(), frame) {
                    Some(origin) => joins.push(PopupJoin {
                        popup: popup.surface,
                        roots: menu.roots.clone(),
                        frame,
                        origin,
                    }),
                    None => findings.push(MenuFinding::Unmatched {
                        popup: popup.surface,
                    }),
                }
            }
            _ => findings.push(MenuFinding::Ambiguous {
                popup: popup.surface,
                menus: landed.len(),
            }),
        }
    }
    (joins, findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(node: u64, title: Option<&str>, pid: Option<u32>) -> WindowClaim {
        WindowClaim {
            node: NodeId(node),
            title: title.map(str::to_owned),
            bus_pid: pid,
            active_at: None,
        }
    }

    fn surface(id: u64, title: Option<&str>, pid: Option<u32>) -> SurfaceClaim {
        SurfaceClaim {
            surface: SurfaceId(id),
            title: title.map(str::to_owned),
            pid,
            focused_at: None,
        }
    }

    #[test]
    fn one_process_with_one_window_binds_on_the_pid_alone() {
        let (joins, findings) = join(
            &[window(1, Some("GTK Widget Factory"), Some(9182))],
            &[surface(7, Some("GTK Widget Factory"), Some(9182))],
        );
        assert!(findings.is_empty());
        assert_eq!(
            joins,
            vec![Join {
                node: NodeId(1),
                surface: SurfaceId(7),
                evidence: vec![Evidence::Pid(9182), Evidence::Title],
            }]
        );
    }

    /// The two toolkits in the M2 demo: two processes, one window each, and
    /// nothing to confuse.
    #[test]
    fn two_applications_bind_to_their_own_surfaces() {
        let (joins, findings) = join(
            &[
                window(1, Some("GTK Widget Factory"), Some(100)),
                window(2, Some("Widget Gallery Qt 6.8.2"), Some(200)),
            ],
            &[
                surface(7, Some("Widget Gallery Qt 6.8.2"), Some(200)),
                surface(8, Some("GTK Widget Factory"), Some(100)),
            ],
        );
        assert!(findings.is_empty());
        assert_eq!(joins[0].surface, SurfaceId(8));
        assert_eq!(joins[1].surface, SurfaceId(7));
    }

    /// An unknown pid on either side is not a match, and specifically is not a
    /// match with another unknown. A gate that opens on absence is not a gate.
    #[test]
    fn an_unknown_process_never_agrees_with_anything() {
        let (joins, findings) = join(
            &[window(1, Some("Same"), None)],
            &[surface(7, Some("Same"), None)],
        );
        assert!(joins.is_empty());
        assert_eq!(findings, vec![Finding::Unmatched { node: NodeId(1) }]);
    }

    /// The title agrees and the process does not. That is what a bridge
    /// serving a tree it did not draw looks like from here, and it is reported
    /// rather than resolved in the title's favour.
    #[test]
    fn a_matching_title_over_a_different_process_is_a_finding() {
        let (joins, findings) = join(
            &[window(1, Some("Text Editor"), Some(100))],
            &[surface(7, Some("Text Editor"), Some(200))],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            vec![Finding::Contradiction {
                node: NodeId(1),
                surface: SurfaceId(7),
                bridge_pid: 100,
                host_pid: 200,
            }]
        );
        assert_eq!(
            findings[0].to_string(),
            "window 1 shares a title with surface 7 but the bridge names \
             process 100 where the host names 200"
        );
    }

    /// One application, two windows: the pid cannot separate them, and the
    /// title can.
    #[test]
    fn two_windows_of_one_application_are_separated_by_their_titles() {
        let (joins, findings) = join(
            &[
                window(1, Some("notes.txt"), Some(100)),
                window(2, Some("draft.txt"), Some(100)),
            ],
            &[
                surface(7, Some("draft.txt"), Some(100)),
                surface(8, Some("notes.txt"), Some(100)),
            ],
        );
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(joins[0].surface, SurfaceId(8));
        assert_eq!(joins[1].surface, SurfaceId(7));
    }

    /// Two identically titled windows of one process, told apart by the
    /// compositor having focused one of them just as the bridge reported it
    /// active.
    #[test]
    fn focus_correlation_separates_what_a_title_cannot() {
        let now = Instant::now();
        let mut claim = window(1, Some("Untitled"), Some(100));
        claim.active_at = Some(now);

        let mut recent = surface(7, Some("Untitled"), Some(100));
        recent.focused_at = Some(now - Duration::from_millis(20));
        let mut stale = surface(8, Some("Untitled"), Some(100));
        stale.focused_at = Some(now - Duration::from_secs(30));

        let (joins, findings) = join(&[claim], &[recent, stale]);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(joins[0].surface, SurfaceId(7));
        assert_eq!(
            joins[0].evidence,
            vec![
                Evidence::Pid(100),
                Evidence::Title,
                Evidence::Focus {
                    skew: Duration::from_millis(20)
                },
            ]
        );
    }

    /// And when nothing separates them, the answer is a finding. This is the
    /// case a system that wanted to look capable would guess at.
    #[test]
    fn two_indistinguishable_windows_are_ambiguous_rather_than_guessed() {
        let (joins, findings) = join(
            &[window(1, Some("Untitled"), Some(100))],
            &[
                surface(7, Some("Untitled"), Some(100)),
                surface(8, Some("Untitled"), Some(100)),
            ],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            vec![Finding::Ambiguous {
                node: NodeId(1),
                candidates: vec![SurfaceId(7), SurfaceId(8)],
            }]
        );
        assert_eq!(
            findings[0].to_string(),
            "window 1 matches 2 surfaces equally well"
        );
    }

    /// A focus that arrived a minute before the activation explains nothing.
    #[test]
    fn focus_outside_the_correlation_window_is_not_evidence() {
        let now = Instant::now();
        let mut claim = window(1, None, Some(100));
        claim.active_at = Some(now);
        let mut old = surface(7, None, Some(100));
        old.focused_at = Some(now - Duration::from_secs(60));

        let (joins, _) = join(&[claim], &[old]);
        assert_eq!(
            joins[0].evidence,
            vec![Evidence::Pid(100)],
            "the pid still binds it; the stale focus is not offered as a reason"
        );
    }

    /// Two windows resolving to one surface means at least one is wrong, and
    /// nothing here knows which. Both are demoted.
    #[test]
    fn a_surface_two_windows_both_claim_binds_to_neither() {
        let now = Instant::now();
        let mut first = window(1, Some("Untitled"), Some(100));
        first.active_at = Some(now);
        let mut second = window(2, Some("Untitled"), Some(100));
        second.active_at = Some(now);

        let mut only = surface(7, Some("Untitled"), Some(100));
        only.focused_at = Some(now);

        let (joins, findings) = join(&[first, second], &[only]);
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            vec![
                Finding::Contested {
                    node: NodeId(1),
                    surface: SurfaceId(7),
                },
                Finding::Contested {
                    node: NodeId(2),
                    surface: SurfaceId(7),
                },
            ]
        );
    }

    // # Menus
    //
    // Every shape below is one slice 1 of #112 measured from a real client
    // (`~/perspicax-test/issue-112/FINDINGS.md`): a 400x300 window placed at
    // (2, 26), whose window node GTK puts at (0, 0), and the popups it
    // opened, with their geometry, their shadow, and where the toolkit said
    // the menu's nodes were.

    const PID: u32 = 9182;
    const WINDOW: SurfaceId = SurfaceId(1);

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    /// The window the menus hang under, in the frame its nodes are read in.
    fn under() -> Under {
        Under {
            surface: WINDOW,
            origin: Vec2::ZERO,
            geometry: rect(2.0, 26.0, 400.0, 300.0),
        }
    }

    /// When a popup opened, and a moment after, when its menu was read.
    fn times() -> (Instant, Instant) {
        let opened = Instant::now();
        (opened, opened + Duration::from_millis(5))
    }

    /// A popup of the window whose geometry is `at` in the window's space,
    /// drawn with a shadow of `(left, top, right, bottom)` around it.
    fn popup(id: u64, at: Rect, shadow: (f64, f64, f64, f64), opened: Instant) -> PopupClaim {
        let geometry = at + Vec2::new(2.0, 26.0);
        let (left, top, right, bottom) = shadow;
        PopupClaim {
            surface: SurfaceId(id),
            from: Some(WINDOW),
            pid: Some(PID),
            mapped_at: Some(opened),
            buffer_origin: Vec2::new(geometry.x0 - left, geometry.y0 - top),
            geometry,
            drawn: Rect::new(
                geometry.x0 - left,
                geometry.y0 - top,
                geometry.x1 + right,
                geometry.y1 + bottom,
            ),
        }
    }

    fn menu(roots: &[u64], extent: Rect, under: Option<Under>, seen: Instant) -> MenuClaim {
        MenuClaim {
            roots: roots.iter().copied().map(NodeId).collect(),
            extent,
            pid: Some(PID),
            under,
            seen_at: Some(seen),
            items: None,
        }
    }

    /// GTK 3 draws with a shadow of 6, 5, 6 and 7.
    const GTK3: (f64, f64, f64, f64) = (6.0, 5.0, 6.0, 7.0);
    /// GTK 4 draws with a shadow of 3, 2, 3 and 4.
    const GTK4: (f64, f64, f64, f64) = (3.0, 2.0, 3.0, 4.0);

    fn joined(popups: &[PopupClaim], menus: &[MenuClaim]) -> Vec<(u64, Vec<u64>, Frame, Vec2)> {
        let (joins, findings) = join_popups(popups, menus);
        assert!(findings.is_empty(), "{findings:?}");
        joins
            .into_iter()
            .map(|join| {
                (
                    join.popup.0,
                    join.roots.iter().map(|root| root.0).collect(),
                    join.frame,
                    join.origin,
                )
            })
            .collect()
    }

    /// GTK 3's context menu is an untitled window of its own, its menu
    /// measured from the popup's surface, exactly the popup's window.
    #[test]
    fn a_gtk3_context_menu_binds_in_its_own_frame() {
        let (opened, read) = times();
        let context = popup(5, rect(300.0, 200.0, 102.0, 86.0), GTK3, opened);
        assert_eq!(
            joined(
                &[context],
                &[menu(&[40], rect(6.0, 5.0, 102.0, 86.0), None, read)]
            ),
            [(5, vec![40], Frame::Own, Vec2::new(6.0, 5.0))]
        );
    }

    /// GTK 3's combo list hangs under the combo box, in the window's tree,
    /// and is still measured from its own surface: read in the window's
    /// frame it would sit over the window's top-left corner.
    #[test]
    fn a_gtk3_combo_list_under_its_window_binds_in_its_own_frame() {
        let (opened, read) = times();
        let list = popup(5, rect(77.0, 86.0, 86.0, 218.0), GTK3, opened);
        assert_eq!(
            joined(
                &[list],
                &[menu(
                    &[15],
                    rect(6.0, 5.0, 86.0, 218.0),
                    Some(under()),
                    read
                )]
            ),
            [(5, vec![15], Frame::Own, Vec2::new(6.0, 5.0))]
        );
    }

    /// GTK 4's popover menu is measured in its window's node space, and its
    /// node is the popup's whole picture, arrow and shadow included.
    #[test]
    fn a_gtk4_popover_binds_in_its_window_frame() {
        let (opened, read) = times();
        let popover = popup(5, rect(-14.0, 300.0, 106.0, 174.0), GTK4, opened);
        assert_eq!(
            joined(
                &[popover],
                &[menu(
                    &[10],
                    rect(-17.0, 298.0, 112.0, 180.0),
                    Some(under()),
                    read
                )]
            ),
            [(5, vec![10], Frame::Window(WINDOW), Vec2::new(-14.0, 300.0))]
        );
    }

    /// GTK 4's drop-down list is neither its popup's window nor its picture:
    /// the picture's width and the window's height. It covers 99% of the
    /// window, inside the picture, and binds.
    #[test]
    fn a_gtk4_drop_down_list_binds_in_its_window_frame() {
        let (opened, read) = times();
        let list = popup(5, rect(86.0, 300.0, 97.0, 255.0), GTK4, opened);
        assert_eq!(
            joined(
                &[list],
                &[menu(
                    &[11],
                    rect(83.0, 298.0, 103.0, 255.0),
                    Some(under()),
                    read
                )]
            ),
            [(5, vec![11], Frame::Window(WINDOW), Vec2::new(86.0, 300.0))]
        );
    }

    /// GTK 3's menu-bar menu has no node of its own: the bar item is on the
    /// bar, in the window's coordinates, and its items are in the menu's. A
    /// submenu's are the same, under the item that opened it. Each popup
    /// takes the items that land on it, the menu's first, so that the
    /// submenu's, beneath them, end up on the submenu.
    #[test]
    fn a_gtk3_menu_bar_menu_and_its_submenu_bind_their_items() {
        let (opened, read) = times();
        let file = popup(5, rect(0.0, 26.0, 121.0, 78.0), GTK3, opened);
        let more = popup(6, rect(119.0, 78.0, 113.0, 52.0), GTK3, opened);
        let bar_item = MenuClaim {
            items: Some(Box::new(menu(
                &[21, 22, 23],
                rect(6.0, 5.0, 121.0, 78.0),
                Some(under()),
                read,
            ))),
            ..menu(&[20], rect(0.0, 0.0, 39.0, 26.0), Some(under()), read)
        };
        let more_item = MenuClaim {
            items: Some(Box::new(menu(
                &[31, 32],
                rect(6.0, 5.0, 113.0, 52.0),
                Some(under()),
                read,
            ))),
            ..menu(&[23], rect(6.0, 57.0, 121.0, 26.0), Some(under()), read)
        };
        assert_eq!(
            joined(&[file, more], &[bar_item, more_item]),
            [
                (5, vec![21, 22, 23], Frame::Own, Vec2::new(6.0, 5.0)),
                (6, vec![31, 32], Frame::Own, Vec2::new(6.0, 5.0)),
            ]
        );
    }

    /// A menu that lands on its popup read either way says nothing about
    /// which way its toolkit measures, and is not bound.
    #[test]
    fn a_menu_landing_in_both_frames_is_not_bound() {
        let (opened, read) = times();
        // At the window's own corner, with no shadow: both frames agree.
        let corner = popup(5, rect(0.0, 0.0, 100.0, 80.0), (0.0, 0.0, 0.0, 0.0), opened);
        let (joins, findings) = join_popups(
            &[corner],
            &[menu(
                &[10],
                rect(0.0, 0.0, 100.0, 80.0),
                Some(under()),
                read,
            )],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            [MenuFinding::Unmatched {
                popup: SurfaceId(5)
            }]
        );
    }

    /// One item of a menu lands inside it but is not it: a popup is bound to
    /// what covers nearly all of it.
    #[test]
    fn a_single_item_is_not_its_menu() {
        let (opened, read) = times();
        let context = popup(5, rect(300.0, 200.0, 102.0, 86.0), GTK3, opened);
        let (joins, _) = join_popups(
            &[context],
            &[menu(&[41], rect(6.0, 9.0, 102.0, 26.0), None, read)],
        );
        assert!(joins.is_empty());
    }

    /// A menu read before its popup opened may be one that has since closed
    /// in its place, with the same size and the same frame: a submenu the
    /// pointer has just left for the next. It is not bound.
    #[test]
    fn a_menu_read_before_its_popup_opened_is_not_bound() {
        let read = Instant::now();
        let opened = read + Duration::from_millis(5);
        let context = popup(5, rect(300.0, 200.0, 102.0, 86.0), GTK3, opened);
        let (joins, findings) = join_popups(
            &[context],
            &[menu(&[40], rect(6.0, 5.0, 102.0, 86.0), None, read)],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            [MenuFinding::Unmatched {
                popup: SurfaceId(5)
            }]
        );
    }

    /// Another process's menu, or a menu under another window, is never a
    /// popup's, wherever it lands.
    #[test]
    fn a_menu_of_another_process_or_window_is_not_bound() {
        let (opened, read) = times();
        let popover = popup(5, rect(-14.0, 300.0, 106.0, 174.0), GTK4, opened);
        let elsewhere = Under {
            surface: SurfaceId(2),
            ..under()
        };
        let menus = [
            MenuClaim {
                pid: Some(PID + 1),
                ..menu(&[10], rect(-17.0, 298.0, 112.0, 180.0), Some(under()), read)
            },
            menu(
                &[11],
                rect(-17.0, 298.0, 112.0, 180.0),
                Some(elsewhere),
                read,
            ),
        ];
        let (joins, _) = join_popups(&[popover], &menus);
        assert!(joins.is_empty());
    }

    /// Two menus that both land on one popup, or one menu that lands on two
    /// popups, bind nothing: which is which is not this join's to guess.
    #[test]
    fn a_popup_two_menus_land_on_or_a_menu_landing_on_two_binds_nothing() {
        let (opened, read) = times();
        let context = popup(5, rect(300.0, 200.0, 102.0, 86.0), GTK3, opened);
        let (joins, findings) = join_popups(
            core::slice::from_ref(&context),
            &[
                menu(&[40], rect(6.0, 5.0, 102.0, 86.0), None, read),
                menu(&[50], rect(6.0, 5.0, 102.0, 86.0), None, read),
            ],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            [MenuFinding::Ambiguous {
                popup: SurfaceId(5),
                menus: 2
            }]
        );

        // The same size, elsewhere: measured from its own surface, the one
        // menu lands on either.
        let twin = popup(6, rect(20.0, 20.0, 102.0, 86.0), GTK3, opened);
        let (joins, findings) = join_popups(
            &[context, twin],
            &[menu(&[40], rect(6.0, 5.0, 102.0, 86.0), None, read)],
        );
        assert!(joins.is_empty());
        assert_eq!(findings.len(), 2);
    }

    // X11 menus, as slice 1 of #159 measured them
    // (`~/perspicax-test/issue-159/FINDINGS.md`): drawn with no shadow, the X
    // window exactly the menu. GTK 3's name their window in
    // `WM_TRANSIENT_FOR`; GTK 4's name none, and are popups of nothing.

    /// An X11 menu as a popup: `at` in the window's space, no shadow, hung
    /// from `from`.
    fn x11_menu(id: u64, at: Rect, from: Option<SurfaceId>, opened: Instant) -> PopupClaim {
        PopupClaim {
            from,
            ..popup(id, at, (0.0, 0.0, 0.0, 0.0), opened)
        }
    }

    /// GTK 3's context menu under X11: its own untitled window, the menu
    /// measured from the X window's corner, which is the menu's. Named or
    /// not, the window it was opened from is not what binds it.
    #[test]
    fn an_x11_menu_window_of_its_own_binds_whatever_it_hangs_from() {
        let (opened, read) = times();
        for from in [Some(WINDOW), None] {
            let context = x11_menu(5, rect(300.0, 200.0, 102.0, 86.0), from, opened);
            assert_eq!(
                joined(
                    &[context],
                    &[menu(&[40], rect(0.0, 0.0, 102.0, 86.0), None, read)]
                ),
                [(5, vec![40], Frame::Own, Vec2::ZERO)],
                "from {from:?}"
            );
        }
    }

    /// GTK 4's popover under X11 hangs from nothing, and its menu hangs under
    /// the window, measured in the window's frame, landing on it exactly. A
    /// popup of nothing is no window's, so it is not bound: which window's
    /// menu it is, nothing says.
    #[test]
    fn a_popup_of_nothing_binds_no_menu_under_a_window() {
        let (opened, read) = times();
        let popover = x11_menu(5, rect(23.0, 298.0, 109.0, 180.0), None, opened);
        let (joins, findings) = join_popups(
            core::slice::from_ref(&popover),
            &[menu(
                &[10],
                rect(23.0, 298.0, 109.0, 180.0),
                Some(under()),
                read,
            )],
        );
        assert!(joins.is_empty());
        assert_eq!(
            findings,
            [MenuFinding::Unmatched {
                popup: SurfaceId(5)
            }]
        );

        // Hung from its window, the same menu binds.
        let named = PopupClaim {
            from: Some(WINDOW),
            ..popover
        };
        assert_eq!(
            joined(
                &[named],
                &[menu(
                    &[10],
                    rect(23.0, 298.0, 109.0, 180.0),
                    Some(under()),
                    read
                )]
            ),
            [(5, vec![10], Frame::Window(WINDOW), Vec2::new(23.0, 298.0))]
        );
    }
}
