//! Which of an application's nodes are an open menu's, and where that menu
//! begins in their coordinates (issue #112).
//!
//! A window's nodes are joined to it whole ([`Index::join_subtree`]). A menu
//! is a surface of its own, an `xdg_popup` drawn directly above the window it
//! hangs from, and its nodes hang somewhere in its application's tree: under
//! that window, or beside it as a window of their own.
//! [`join_popups`](crate::join::join_popups) decides which part of the tree
//! each open menu is. This is the index's half: gathering what could be a
//! menu, binding what was decided, checking at every judgement that it still
//! holds, and refusing a menu's nodes while one is open and nothing is bound
//! to it.

use std::collections::{HashMap, HashSet};

use perspicax_node::{NodeId, Rect, Role, SurfaceId, Vec2};

use super::Index;
use crate::{
    host::{HostFacts, SurfaceFacts},
    join::{self, Frame, MenuClaim, MenuFinding, PopupJoin, Under},
};

impl Index {
    /// Bind each open popup an application drew to the part of its tree that
    /// is that menu, and say why any could not be.
    ///
    /// `windows` are the application's windows as the window join bound them,
    /// each as its window node and surface. Call this after joining those: a
    /// menu under a window may be measured in that window's coordinates, and
    /// the window's walk joins every node beneath it, its menus' included,
    /// which this then joins to their popups again.
    ///
    /// A popup bound before and not bound now is unbound, and the nodes that
    /// were joined to it are marked stale: their coordinates may be the
    /// menu's own, which on the window they go back to would put them
    /// somewhere else entirely, so they wait for a read.
    pub fn join_popups(
        &mut self,
        root: NodeId,
        pid: u32,
        windows: &[(NodeId, SurfaceId)],
        facts: &HostFacts,
    ) -> Vec<MenuFinding> {
        let popups: Vec<_> = facts
            .surfaces()
            .iter()
            .filter_map(SurfaceFacts::popup_claim)
            .filter(|popup| popup.pid == Some(pid))
            .collect();
        let menus = self.menu_claims(root, pid, windows, facts);
        let (joins, findings) = join::join_popups(&popups, &menus);

        let rebound: Vec<SurfaceId> = self
            .menus
            .values()
            .filter(|old| {
                popups.iter().any(|popup| popup.surface == old.popup)
                    && !joins.iter().any(|new| {
                        new.popup == old.popup && new.roots == old.roots && new.frame == old.frame
                    })
            })
            .map(|old| old.popup)
            .collect();
        for popup in rebound {
            self.unbind(popup);
        }
        for join in joins {
            self.menus.insert(join.popup, join);
        }
        self.restamp_menus(facts);
        findings
    }

    /// Join each bound menu's nodes to its popup again, bottom to top, after
    /// a window's join has taken them back. A submenu's nodes hang beneath
    /// its menu's, so its popup, which is above its menu's, takes them last.
    pub fn restamp_menus(&mut self, facts: &HostFacts) {
        let order: Vec<_> = facts
            .surfaces()
            .iter()
            .filter_map(|surface| {
                let join = self.menus.get(&surface.id)?;
                Some((surface.id, join.roots.clone(), surface.origin.clone()))
            })
            .collect();
        for (popup, roots, origin) in order {
            for root in roots {
                self.join_subtree(root, popup, &origin);
            }
        }
    }

    /// The menu bound to this popup, if one is.
    #[must_use]
    pub fn menu_of(&self, popup: SurfaceId) -> Option<&PopupJoin> {
        self.menus.get(&popup)
    }

    /// Check every bound menu against the facts it is about to be judged by:
    /// still on screen, still landing on its popup in the frame it was bound
    /// in. Where its popup is in that frame is measured again, so a menu its
    /// client moved is judged where it is now. One that no longer lands is
    /// unbound.
    pub(super) fn refresh_menus(&mut self, facts: &HostFacts) {
        let checked: Vec<(SurfaceId, Option<Vec2>)> = self
            .menus
            .values()
            .map(|join| (join.popup, self.still_lands(join, facts)))
            .collect();
        for (popup, origin) in checked {
            match (origin, self.menus.get_mut(&popup)) {
                (Some(origin), Some(join)) => join.origin = origin,
                _ => self.unbind(popup),
            }
        }
    }

    /// Where `join`'s popup begins in its menu's coordinates now, if the menu
    /// still lands on it.
    fn still_lands(&self, join: &PopupJoin, facts: &HostFacts) -> Option<Vec2> {
        let popup = facts.surface(join.popup)?.popup_claim()?;
        let under = match join.frame {
            Frame::Own => None,
            Frame::Window(window) => Some(Under {
                surface: window,
                origin: self.window_origin(window)?,
                geometry: facts.surface(window)?.geometry,
            }),
        };
        let menu = MenuClaim {
            roots: join.roots.clone(),
            extent: self.extent(&join.roots)?,
            pid: None,
            under,
            seen_at: None,
            items: None,
        };
        join::lands_on(&popup, &menu, join.frame)
            .then(|| join::popup_origin(&popup, under.as_ref(), join.frame))
            .flatten()
    }

    /// Forget the menu bound to `popup`, and mark what was joined to it stale
    /// until a read: see [`Index::join_popups`].
    fn unbind(&mut self, popup: SurfaceId) {
        let Some(join) = self.menus.remove(&popup) else {
            return;
        };
        self.windows.remove(&popup);
        for root in join.roots {
            self.invalidate(root);
        }
    }

    /// Every node whose window, or a menu of that window, has a menu open
    /// that no part of its tree is bound to, and belongs to a menu itself:
    /// refused as [`MenuUnplaced`](crate::Refusal::MenuUnplaced), naming the
    /// open menu. Only nodes their application says are shown: a hidden one
    /// is refused for that, which says more.
    ///
    /// A popup of nothing names no window, so it holds off every window of
    /// its application: its nodes could hang under any of them, measured from
    /// the menu itself (issue #159).
    pub(super) fn unplaced(&self, facts: &HostFacts) -> HashMap<NodeId, SurfaceId> {
        let mut open: HashMap<SurfaceId, SurfaceId> = HashMap::new();
        let mut of_nothing: HashMap<u32, SurfaceId> = HashMap::new();
        for surface in facts.surfaces() {
            if !surface.is_popup() || !surface.mapped || self.menus.contains_key(&surface.id) {
                continue;
            }
            match (surface.hangs_from(), surface.claim().pid) {
                (Some(from), _) => {
                    open.entry(from).or_insert(surface.id);
                }
                (None, Some(pid)) => {
                    of_nothing.entry(pid).or_insert(surface.id);
                }
                // No window, and no application to say whose: nothing here
                // could have been measured from it.
                (None, None) => {}
            }
        }
        if open.is_empty() && of_nothing.is_empty() {
            return HashMap::new();
        }
        self.nodes
            .iter()
            .filter_map(|(id, node)| {
                let surface = node.surface?;
                let described = facts.surface(surface);
                let window = described
                    .and_then(SurfaceFacts::hangs_from)
                    .unwrap_or(surface);
                let popup = *open
                    .get(&window)
                    .or_else(|| of_nothing.get(&described?.claim().pid?))?;
                (!node.node.is_hidden() && self.in_menu(*id)).then_some((*id, popup))
            })
            .collect()
    }

    /// Whether a node is part of a menu: it or something above it is one.
    fn in_menu(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        let mut seen = HashSet::new();
        while let Some(id) = at.filter(|id| seen.insert(*id)) {
            if self.menu_like(id) {
                return true;
            }
            at = self.parents.get(&id).copied();
        }
        false
    }

    /// Whether a node is a menu, or a combo box's list: what an open popup's
    /// nodes are under, in every toolkit measured.
    fn menu_like(&self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(&id) else {
            return false;
        };
        match node.node.role() {
            Role::Menu => true,
            Role::ListBox | Role::List | Role::Tree => self
                .parents
                .get(&id)
                .and_then(|parent| self.nodes.get(parent))
                .is_some_and(|parent| parent.node.role() == Role::ComboBox),
            _ => false,
        }
    }

    /// Everything in an application's tree that could be one of its open
    /// menus: each menu or combo list under one of its `windows`, read in
    /// either frame, and each window of its own that is a menu, read in its
    /// own.
    fn menu_claims(
        &self,
        root: NodeId,
        pid: u32,
        windows: &[(NodeId, SurfaceId)],
        facts: &HostFacts,
    ) -> Vec<MenuClaim> {
        let mut claims = Vec::new();
        let mut claimed = HashSet::new();
        for &(window, surface) in windows {
            let (Some(origin), Some(facts)) = (self.window_origin(surface), facts.surface(surface))
            else {
                continue;
            };
            let under = Under {
                surface,
                origin,
                geometry: facts.geometry,
            };
            for id in self.descendants(window) {
                if self.menu_like(id) && claimed.insert(id) {
                    claims.extend(self.menu_claim(id, pid, Some(under)));
                }
            }
        }

        // A menu that is a window of its own, as GTK 3 makes a context menu
        // and an open combo list: a child of the application, or a window
        // whose child it is.
        let joined: HashSet<NodeId> = windows.iter().map(|(window, _)| *window).collect();
        let children = self
            .nodes
            .get(&root)
            .map(|app| app.node.children().to_vec())
            .unwrap_or_default();
        for child in children.into_iter().filter(|child| !joined.contains(child)) {
            if let Some(menu) = self.menu_window(child)
                && claimed.insert(menu)
            {
                claims.extend(self.menu_claim(menu, pid, None));
            }
        }

        // A menu inside another with exactly its extent is the same menu
        // measured twice: keep the outer.
        let doubled: Vec<usize> = claims
            .iter()
            .enumerate()
            .filter(|(_, inner)| {
                claims.iter().any(|outer| {
                    outer.extent == inner.extent
                        && outer.roots != inner.roots
                        && inner
                            .roots
                            .iter()
                            .all(|root| outer.roots.iter().any(|o| self.descends(*root, *o)))
                })
            })
            .map(|(index, _)| index)
            .collect();
        claims
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !doubled.contains(index))
            .map(|(_, claim)| claim)
            .collect()
    }

    /// The menu an application's child is, if it is one, or is an untitled
    /// window holding nothing but one, which is how GTK 3 puts a context menu
    /// or an open combo list beside the application's windows. Never a window
    /// with a title or anything else in it: that is an application's window,
    /// and taking it for a menu would leave it unjoined.
    #[must_use]
    pub fn menu_window(&self, child: NodeId) -> Option<NodeId> {
        let node = self.nodes.get(&child)?;
        match (node.node.role(), node.node.children()) {
            (Role::Menu, _) => Some(child),
            (Role::Window, [only])
                if node.node.label().is_none_or(str::is_empty)
                    && self
                        .nodes
                        .get(only)
                        .is_some_and(|only| only.node.role() == Role::Menu) =>
            {
                Some(*only)
            }
            _ => None,
        }
    }

    /// One menu as a claim, if it is shown and not stale: its own extent,
    /// with its shown items as the claim to make instead.
    fn menu_claim(&self, id: NodeId, pid: u32, under: Option<Under>) -> Option<MenuClaim> {
        let node = self.nodes.get(&id)?;
        if node.node.is_hidden() || self.stale.contains_key(&id) {
            return None;
        }
        let items: Vec<NodeId> = node
            .node
            .children()
            .iter()
            .copied()
            .filter(|item| {
                !self.stale.contains_key(item)
                    && self
                        .nodes
                        .get(item)
                        .is_some_and(|item| !item.node.is_hidden())
            })
            .collect();
        let items = self.claim_of(items, pid, under).map(Box::new);
        match self.claim_of(vec![id], pid, under) {
            Some(own) => Some(MenuClaim { items, ..own }),
            None => items.map(|items| *items),
        }
    }

    /// `roots` as a claim, if they reach anywhere.
    fn claim_of(&self, roots: Vec<NodeId>, pid: u32, under: Option<Under>) -> Option<MenuClaim> {
        let extent = self.extent(&roots)?;
        let seen_at = roots
            .iter()
            .map(|root| self.seen.get(root).copied())
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .min();
        Some(MenuClaim {
            roots,
            extent,
            pid: Some(pid),
            under,
            seen_at,
            items: None,
        })
    }

    /// Where `roots` reach together in node space, counting only those with
    /// an area. `None` if none has one.
    fn extent(&self, roots: &[NodeId]) -> Option<Rect> {
        roots
            .iter()
            .filter_map(|root| self.nodes.get(root)?.node_space_bounds())
            .filter(|bounds| !bounds.abs().is_empty())
            .reduce(|extent, bounds| extent.union(bounds))
    }

    /// Whether `id` lies beneath `ancestor`.
    fn descends(&self, id: NodeId, ancestor: NodeId) -> bool {
        let mut at = self.parents.get(&id).copied();
        let mut seen = HashSet::new();
        while let Some(parent) = at.filter(|parent| seen.insert(*parent)) {
            if parent == ancestor {
                return true;
            }
            at = self.parents.get(&parent).copied();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use perspicax_node::{Node, ObservedNode, Origin, ProcessOrigin, Visibility};

    use super::*;
    use crate::{Consent, Delta, Refusal, host::SurfaceFacts};

    // The shapes slice 1 of #112 measured from GTK 3 and GTK 4
    // (`~/perspicax-test/issue-112/FINDINGS.md`): a 400x300 window placed at
    // (2, 26), its window node at (0, 0), and menus opened from it.

    const PID: u32 = 9182;
    const APP: NodeId = NodeId(100);
    const WINDOW: SurfaceId = SurfaceId(1);
    const POPUP: SurfaceId = SurfaceId(5);

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    fn process(pid: u32) -> Origin {
        Origin::Process(Box::new(ProcessOrigin {
            pid,
            exe: Some("/usr/bin/python3".into()),
            cgroup: None,
            sandbox: None,
        }))
    }

    fn node(id: u64, role: Role, bounds: Option<Rect>, children: &[u64]) -> ObservedNode {
        let mut node = Node::new(role);
        if let Some(bounds) = bounds {
            node.set_bounds(bounds);
        }
        node.set_children(children.iter().map(|c| NodeId(*c)).collect::<Vec<_>>());
        ObservedNode::unjoined(NodeId(id), node)
    }

    /// The window, at (2, 26), drawn by the application.
    fn window() -> SurfaceFacts {
        SurfaceFacts::new(WINDOW, rect(2.0, 26.0, 400.0, 300.0)).owned_by(process(PID))
    }

    /// A popup of the window whose geometry is `at` in the window's space,
    /// with GTK's shadow of `(left, top, right, bottom)` around it.
    fn popup(at: Rect, shadow: (f64, f64, f64, f64), opened: Instant) -> SurfaceFacts {
        let geometry = at + Vec2::new(2.0, 26.0);
        let (left, top, right, bottom) = shadow;
        SurfaceFacts::new(POPUP, geometry)
            .with_buffer_origin(Vec2::new(geometry.x0 - left, geometry.y0 - top))
            .drawing_over(Rect::new(
                geometry.x0 - left,
                geometry.y0 - top,
                geometry.x1 + right,
                geometry.y1 + bottom,
            ))
            .hung_from(WINDOW, opened)
            .owned_by(process(PID))
    }

    fn desk(surfaces: impl IntoIterator<Item = SurfaceFacts>) -> HostFacts {
        HostFacts::bottom_to_top(surfaces, 1).with_consent(Consent::Everyone)
    }

    /// GTK 4's popover, opened from a button at the foot of the window: the
    /// menu, measured in the window's node space, hangs below its edge.
    /// "Above" is an item over the window, and "Below" one past its edge.
    const GTK4_POPOVER: (f64, f64, f64, f64) = (-14.0, 300.0, 106.0, 174.0);

    fn gtk4(opened: Instant) -> (Index, HostFacts) {
        let mut index = Index::new();
        index.ingest_snapshot([
            node(100, Role::Application, None, &[1]),
            node(1, Role::Window, Some(rect(0.0, 0.0, 400.0, 300.0)), &[2, 7]),
            node(2, Role::Button, Some(rect(0.0, 266.0, 78.0, 34.0)), &[3]),
            node(
                3,
                Role::Menu,
                Some(rect(-17.0, 298.0, 112.0, 180.0)),
                &[4, 5],
            ),
            node(4, Role::MenuItem, Some(rect(-14.0, 312.0, 94.0, 30.0)), &[]),
            node(5, Role::MenuItem, Some(rect(-14.0, 432.0, 94.0, 30.0)), &[]),
            node(7, Role::Label, Some(rect(10.0, 10.0, 60.0, 20.0)), &[]),
        ]);
        index.join_subtree(NodeId(1), WINDOW, &process(PID));
        let (x, y, w, h) = GTK4_POPOVER;
        let facts = desk([
            window(),
            popup(rect(x, y, w, h), (3.0, 2.0, 3.0, 4.0), opened),
        ]);
        (index, facts)
    }

    fn bind(index: &mut Index, facts: &HostFacts) -> Vec<MenuFinding> {
        index.join_popups(APP, PID, &[(NodeId(1), WINDOW)], facts)
    }

    /// Issue #112's second failure: an item of a menu hanging past its
    /// window's edge was refused as clipped. Joined to its popup, it is
    /// measured from the popup and judged inside it.
    #[test]
    fn a_menu_item_past_its_windows_edge_is_judged_on_its_menu() {
        let opened = Instant::now();
        let (mut index, facts) = gtk4(opened);
        assert!(bind(&mut index, &facts).is_empty());
        index.judge(&facts);

        let below = index.get(NodeId(5)).unwrap();
        assert_eq!(below.surface, Some(POPUP));
        assert_eq!(index.window_origin(POPUP), Some(Vec2::new(-14.0, 300.0)));
        assert_eq!(
            index.window_bounds(NodeId(5)),
            Some(rect(0.0, 132.0, 94.0, 30.0)),
            "measured from the popup's own corner"
        );
        assert!(index.actable(NodeId(5)).is_ok());
        assert_eq!(
            index.menu_of(POPUP).map(|menu| menu.roots.clone()),
            Some(vec![NodeId(3)])
        );
        assert!(
            index.actable(NodeId(7)).is_ok(),
            "the window's own nodes are its own"
        );
    }

    /// Issue #112's first failure, on the window's own nodes: a button under
    /// its window's own open menu was judged visible, and the menu took the
    /// click. The menu is drawn directly above its window, and covers it.
    #[test]
    fn a_windows_own_node_under_its_open_menu_is_covered_by_it() {
        let opened = Instant::now();
        let mut index = Index::new();
        index.ingest_snapshot([
            node(100, Role::Application, None, &[1]),
            node(1, Role::Window, Some(rect(0.0, 0.0, 400.0, 300.0)), &[2, 7]),
            node(2, Role::Button, Some(rect(0.0, 266.0, 78.0, 34.0)), &[3]),
            node(3, Role::Menu, Some(rect(97.0, 98.0, 112.0, 180.0)), &[4]),
            node(4, Role::MenuItem, Some(rect(100.0, 112.0, 94.0, 30.0)), &[]),
            node(7, Role::Button, Some(rect(120.0, 150.0, 40.0, 20.0)), &[]),
        ]);
        index.join_subtree(NodeId(1), WINDOW, &process(PID));
        let facts = desk([
            window(),
            popup(
                rect(100.0, 100.0, 106.0, 174.0),
                (3.0, 2.0, 3.0, 4.0),
                opened,
            ),
        ]);
        assert!(bind(&mut index, &facts).is_empty());
        index.judge(&facts);

        assert_eq!(
            index.actable(NodeId(7)).unwrap_err(),
            Refusal::Occluded { by: POPUP }
        );
        assert!(
            index.actable(NodeId(4)).is_ok(),
            "the menu's own item is not"
        );
    }

    /// And on another application's: a menu covers whatever is under it.
    #[test]
    fn another_windows_node_under_a_menu_is_covered_by_it() {
        let opened = Instant::now();
        let (mut index, _) = gtk4(opened);
        index.ingest_snapshot([
            node(200, Role::Application, None, &[20]),
            node(20, Role::Window, Some(rect(0.0, 0.0, 300.0, 300.0)), &[21]),
            node(21, Role::Button, Some(rect(10.0, 20.0, 40.0, 20.0)), &[]),
        ]);
        index.join_subtree(NodeId(20), SurfaceId(2), &process(PID + 1));
        // Beneath the window, its button under where the menu hangs.
        let beneath = SurfaceFacts::new(SurfaceId(2), rect(-30.0, 330.0, 300.0, 300.0))
            .owned_by(process(PID + 1));
        let (x, y, w, h) = GTK4_POPOVER;
        let facts = desk([
            beneath,
            window(),
            popup(rect(x, y, w, h), (3.0, 2.0, 3.0, 4.0), opened),
        ]);
        index.judge(&facts);
        assert_eq!(
            index.actable(NodeId(21)).unwrap_err(),
            Refusal::Occluded { by: POPUP }
        );
    }

    /// While a window has a menu open that no part of its tree is bound to,
    /// its menus' nodes are refused, naming the open menu: where they are
    /// cannot be said, and GTK 3's, read on the window, would be over its
    /// top-left corner. The rest of the window is judged as ever.
    #[test]
    fn while_a_menu_is_open_and_unbound_its_windows_menus_are_refused() {
        let opened = Instant::now();
        let (mut index, facts) = gtk4(opened);
        index.judge(&facts);

        for item in [3, 4, 5] {
            assert_eq!(
                index.actable(NodeId(item)).unwrap_err(),
                Refusal::MenuUnplaced { popup: POPUP },
                "node {item}"
            );
        }
        assert!(index.actable(NodeId(7)).is_ok());

        // With no menu open, the window's menus are judged as ever.
        index.judge(&desk([window()]));
        assert_ne!(
            index.actable(NodeId(4)).unwrap_err(),
            Refusal::MenuUnplaced { popup: POPUP }
        );
    }

    /// A menu read before its popup opened may be one that has since closed
    /// in its place. It is not bound, and its nodes are refused until a read
    /// after the popup opened says it is still there.
    #[test]
    fn a_menu_read_before_its_popup_opened_is_bound_only_once_read_again() {
        let (mut index, _) = gtk4(Instant::now());
        let opened = Instant::now() + Duration::from_millis(5);
        let (x, y, w, h) = GTK4_POPOVER;
        let facts = desk([
            window(),
            popup(rect(x, y, w, h), (3.0, 2.0, 3.0, 4.0), opened),
        ]);

        assert_eq!(
            bind(&mut index, &facts),
            [MenuFinding::Unmatched { popup: POPUP }]
        );
        index.judge(&facts);
        assert_eq!(
            index.actable(NodeId(5)).unwrap_err(),
            Refusal::MenuUnplaced { popup: POPUP }
        );

        // Read again after it opened: the menu is still there, and is bound.
        index.ingest_read(
            [node(
                3,
                Role::Menu,
                Some(rect(-17.0, 298.0, 112.0, 180.0)),
                &[4, 5],
            )],
            opened + Duration::from_millis(1),
        );
        assert!(bind(&mut index, &facts).is_empty());
        index.judge(&facts);
        assert!(index.actable(NodeId(5)).is_ok());
    }

    /// GTK 3's context menu beside its window, as slice 1 measured it: an
    /// untitled window holding the menu, which is the popup's window exactly,
    /// measured from the popup's own surface; the popup at `at`.
    fn gtk3(opened: Instant, at: Rect) -> (Index, HostFacts) {
        let mut index = Index::new();
        index.ingest_snapshot([
            node(100, Role::Application, None, &[1, 8]),
            node(1, Role::Window, Some(rect(0.0, 0.0, 400.0, 300.0)), &[]),
            node(8, Role::Window, Some(rect(0.0, 0.0, 114.0, 98.0)), &[9]),
            node(9, Role::Menu, Some(rect(6.0, 5.0, 102.0, 86.0)), &[10]),
            node(10, Role::MenuItem, Some(rect(6.0, 9.0, 102.0, 26.0)), &[]),
        ]);
        index.join_subtree(NodeId(1), WINDOW, &process(PID));
        (
            index,
            desk([window(), popup(at, (6.0, 5.0, 6.0, 7.0), opened)]),
        )
    }

    /// GTK 3 puts a context menu beside the application's windows, as an
    /// untitled window of its own holding the menu, measured from the popup's
    /// own surface. It is bound there, and its items are measured from the
    /// popup's corner.
    #[test]
    fn a_gtk3_menu_beside_its_window_is_bound_in_its_own_frame() {
        let opened = Instant::now();
        let (mut index, facts) = gtk3(opened, rect(300.0, 200.0, 102.0, 86.0));
        assert_eq!(index.menu_window(NodeId(8)), Some(NodeId(9)));
        assert!(bind(&mut index, &facts).is_empty());
        index.judge(&facts);

        assert_eq!(index.window_origin(POPUP), Some(Vec2::new(6.0, 5.0)));
        assert_eq!(
            index.window_bounds(NodeId(10)),
            Some(rect(0.0, 4.0, 102.0, 26.0))
        );
        assert!(index.actable(NodeId(10)).is_ok());
    }

    /// A menu its client moves is judged where it is now. GTK 3 measures
    /// from the menu itself, so the same nodes land on the popup wherever it
    /// goes, and are placed from its corner there.
    #[test]
    fn a_menu_measured_from_itself_is_judged_where_it_moved_to() {
        let opened = Instant::now();
        let (mut index, facts) = gtk3(opened, rect(300.0, 200.0, 102.0, 86.0));
        bind(&mut index, &facts);
        let (_, moved) = gtk3(opened, rect(20.0, 40.0, 102.0, 86.0));
        index.judge(&moved);

        assert!(index.menu_of(POPUP).is_some());
        let item = index.window_bounds(NodeId(10)).unwrap();
        assert_eq!(
            moved.surface(POPUP).unwrap().to_global(item),
            rect(22.0, 70.0, 102.0, 26.0),
            "where the moved menu draws it"
        );
        assert!(index.actable(NodeId(10)).is_ok());
    }

    /// GTK 4 measures in its window's frame, so a popup that moved without
    /// its nodes moving with it is no longer where they say: unbound, and
    /// stale until the application is read again.
    #[test]
    fn a_menu_measured_from_its_window_that_moved_without_it_is_unbound() {
        let opened = Instant::now();
        let (mut index, facts) = gtk4(opened);
        bind(&mut index, &facts);
        let moved = desk([
            window(),
            popup(
                rect(120.0, 40.0, 106.0, 174.0),
                (3.0, 2.0, 3.0, 4.0),
                opened,
            ),
        ]);
        index.judge(&moved);
        assert!(index.menu_of(POPUP).is_none());
        assert!(matches!(
            index.actable(NodeId(5)).unwrap_err(),
            Refusal::Stale { .. }
        ));
    }

    /// A menu that closes leaves its nodes stale, not quietly on the window:
    /// GTK 3's would be measured from a menu that is no longer there.
    #[test]
    fn a_menu_that_closed_leaves_its_nodes_stale_until_read() {
        let opened = Instant::now();
        let (mut index, facts) = gtk4(opened);
        bind(&mut index, &facts);
        index.judge(&facts);
        assert!(index.actable(NodeId(5)).is_ok());

        index.judge(&desk([window()]));
        assert!(index.menu_of(POPUP).is_none());
        assert!(matches!(
            index.actable(NodeId(5)).unwrap_err(),
            Refusal::Stale { .. }
        ));
        assert!(index.actable(NodeId(7)).is_ok(), "the window's own are not");
    }

    /// A window's join walks its whole tree, its open menu's nodes included,
    /// and runs on every change. The menu's nodes go back to its popup after
    /// it, or they would be judged on the window from the popup's corner.
    #[test]
    fn a_windows_join_gives_its_menus_nodes_back_to_their_popup() {
        let opened = Instant::now();
        let (mut index, facts) = gtk4(opened);
        bind(&mut index, &facts);
        index.join_subtree(NodeId(1), WINDOW, &process(PID));
        assert_eq!(index.get(NodeId(5)).unwrap().surface, Some(WINDOW));

        index.restamp_menus(&facts);
        assert_eq!(index.get(NodeId(5)).unwrap().surface, Some(POPUP));
        assert_eq!(index.window_origin(POPUP), Some(Vec2::new(-14.0, 300.0)));
    }

    /// Only an untitled window holding nothing but a menu is a menu: a window
    /// with a title, or with more in it, is an application's, and taking it
    /// for a menu would leave it unjoined.
    #[test]
    fn a_window_with_a_title_or_more_in_it_is_not_a_menu() {
        let mut index = Index::new();
        let mut titled = node(8, Role::Window, Some(rect(0.0, 0.0, 114.0, 98.0)), &[9]);
        titled.node.set_label("Settings");
        index.ingest_snapshot([
            titled,
            node(9, Role::Menu, Some(rect(6.0, 5.0, 102.0, 86.0)), &[]),
            node(
                11,
                Role::Window,
                Some(rect(0.0, 0.0, 114.0, 98.0)),
                &[12, 13],
            ),
            node(12, Role::Menu, Some(rect(6.0, 5.0, 102.0, 86.0)), &[]),
            node(13, Role::Button, Some(rect(6.0, 5.0, 10.0, 10.0)), &[]),
        ]);
        assert_eq!(index.menu_window(NodeId(8)), None);
        assert_eq!(index.menu_window(NodeId(11)), None);
    }

    /// A menu is its window's to grant: one opening is not news of a grant,
    /// though a window of the same process mapping would be.
    #[test]
    fn a_menu_opening_is_not_announced_as_a_grant() {
        let opened = Instant::now();
        let (mut index, facts) = gtk4(opened);
        let granted = |facts: HostFacts| facts.with_consent(Consent::granted(vec![PID]));
        index.judge(&granted(desk([window()])));
        index.take_deltas();
        index.judge(&granted(facts));
        assert!(
            !index
                .take_deltas()
                .iter()
                .any(|delta| matches!(delta, Delta::Granted { .. }))
        );
    }

    /// Visibility is set for a node refused as unplaced, so a tally counts
    /// it as unjudged rather than as visible.
    #[test]
    fn an_unplaced_menu_node_is_counted_unjudged() {
        let (mut index, facts) = gtk4(Instant::now());
        let tally = index.judge(&facts);
        assert_eq!(
            index.get(NodeId(4)).unwrap().visibility,
            Visibility::Unknown
        );
        assert!(tally.unjudged >= 3);
    }

    // X11 menus, as slice 1 of #159 measured them
    // (`~/perspicax-test/issue-159/FINDINGS.md`). GTK 4's name no window
    // they were opened from: popups of nothing.

    /// GTK 4's popover as X11 has it: no window named, the X window exactly
    /// the menu's picture, and the menu under the window, in its frame.
    fn gtk4_of_nothing(opened: Instant) -> (Index, SurfaceFacts) {
        let (index, _) = gtk4(opened);
        let (x, y, w, h) = GTK4_POPOVER;
        let popover = popup(
            rect(x - 3.0, y - 2.0, w + 6.0, h + 6.0),
            (0.0, 0.0, 0.0, 0.0),
            opened,
        )
        .hung_from_nothing(opened);
        (index, popover)
    }

    /// Another application's window beside the first, with a menu of its
    /// own in its tree.
    fn another_with_a_menu(index: &mut Index) -> SurfaceFacts {
        index.ingest_snapshot([
            node(200, Role::Application, None, &[20]),
            node(20, Role::Window, Some(rect(0.0, 0.0, 300.0, 300.0)), &[21]),
            node(21, Role::Menu, Some(rect(10.0, 20.0, 40.0, 20.0)), &[]),
        ]);
        index.join_subtree(NodeId(20), SurfaceId(2), &process(PID + 1));
        SurfaceFacts::new(SurfaceId(2), rect(600.0, 26.0, 300.0, 300.0)).owned_by(process(PID + 1))
    }

    /// A popup of nothing names no window to hold off, so while it is open
    /// and unbound it holds off every menu of its application, and only its
    /// application's. Its menu hangs under a window, so nothing binds it: a
    /// GTK 4 application's menus stay refused under X11, as decided (#159).
    #[test]
    fn while_a_popup_of_nothing_is_open_and_unbound_its_applications_menus_are_refused() {
        let opened = Instant::now();
        let (mut index, popover) = gtk4_of_nothing(opened);
        let beside = another_with_a_menu(&mut index);
        let facts = desk([beside, window(), popover]);
        assert_eq!(
            bind(&mut index, &facts),
            [MenuFinding::Unmatched { popup: POPUP }]
        );
        index.judge(&facts);

        for item in [3, 4, 5] {
            assert_eq!(
                index.actable(NodeId(item)).unwrap_err(),
                Refusal::MenuUnplaced { popup: POPUP },
                "node {item}"
            );
        }
        assert!(
            index.actable(NodeId(7)).is_ok(),
            "its window's own nodes are judged as ever"
        );
        assert!(
            index.actable(NodeId(21)).is_ok(),
            "another application's menu is not its to hold"
        );
    }

    /// One no application owns holds off nothing: there is no saying whose
    /// menus it could be.
    #[test]
    fn a_popup_of_nothing_nobody_owns_holds_off_no_menu() {
        let opened = Instant::now();
        let (mut index, popover) = gtk4_of_nothing(opened);
        let facts = desk([window(), popover.owned_by(Origin::Unattributed)]);
        index.judge(&facts);
        assert_ne!(
            index.actable(NodeId(3)).unwrap_err(),
            Refusal::MenuUnplaced { popup: POPUP }
        );
    }

    /// GTK 3's context menu as X11 has it: no shadow, the X window exactly
    /// the menu, which is an untitled window of its own measured from it. A
    /// menu that is a window of its own binds to a popup of nothing, and its
    /// items are measured from the menu's corner.
    #[test]
    fn a_popup_of_nothing_is_bound_to_a_menu_that_is_a_window_of_its_own() {
        let opened = Instant::now();
        let mut index = Index::new();
        index.ingest_snapshot([
            node(100, Role::Application, None, &[1, 8]),
            node(1, Role::Window, Some(rect(0.0, 0.0, 400.0, 300.0)), &[]),
            node(8, Role::Window, Some(rect(0.0, 0.0, 102.0, 86.0)), &[9]),
            node(9, Role::Menu, Some(rect(0.0, 0.0, 102.0, 86.0)), &[10]),
            node(10, Role::MenuItem, Some(rect(0.0, 30.0, 102.0, 26.0)), &[]),
        ]);
        index.join_subtree(NodeId(1), WINDOW, &process(PID));
        let context = popup(
            rect(300.0, 200.0, 102.0, 86.0),
            (0.0, 0.0, 0.0, 0.0),
            opened,
        )
        .hung_from_nothing(opened);
        let facts = desk([window(), context]);
        assert!(bind(&mut index, &facts).is_empty());
        index.judge(&facts);

        assert_eq!(index.window_origin(POPUP), Some(Vec2::ZERO));
        assert_eq!(
            index.window_bounds(NodeId(10)),
            Some(rect(0.0, 30.0, 102.0, 26.0))
        );
        assert!(index.actable(NodeId(10)).is_ok());
    }

    /// A popup of nothing is no window, so one opening is no news of a grant
    /// either.
    #[test]
    fn a_popup_of_nothing_opening_is_not_announced_as_a_grant() {
        let opened = Instant::now();
        let (mut index, popover) = gtk4_of_nothing(opened);
        let granted = |facts: HostFacts| facts.with_consent(Consent::granted(vec![PID]));
        index.judge(&granted(desk([window()])));
        index.take_deltas();
        index.judge(&granted(desk([window(), popover])));
        assert!(
            !index
                .take_deltas()
                .iter()
                .any(|delta| matches!(delta, Delta::Granted { .. }))
        );
    }
}
