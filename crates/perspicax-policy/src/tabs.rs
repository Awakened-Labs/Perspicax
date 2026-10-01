//! Tab groups: several windows sharing one place on screen, one of them
//! showing at a time, as Fluxbox groups them.
//!
//! A group is its members in tab order and which of them is in front. Only
//! the one in front is on screen. The rest keep their place in the group and
//! nothing else: where the group is, how big, and on which workspace, is the
//! front tab's, and a tab brought to the front takes them over.
//!
//! A group of one is no group: a window detached from a pair leaves the other
//! on its own, ungrouped, as if it had never been grouped.

/// Every tab group.
#[derive(Debug, Clone)]
pub struct Groups<W> {
    groups: Vec<Group<W>>,
}

#[derive(Debug, Clone)]
struct Group<W> {
    /// In tab order, left to right. Always two or more.
    members: Vec<W>,
    /// Which member is in front.
    front: usize,
}

impl<W> Default for Groups<W> {
    fn default() -> Self {
        Self { groups: Vec::new() }
    }
}

impl<W: Copy + PartialEq> Groups<W> {
    fn find(&self, window: W) -> Option<usize> {
        self.groups
            .iter()
            .position(|group| group.members.contains(&window))
    }

    /// The tabs of `window`'s group in order, or `None` if it is in none.
    #[must_use]
    pub fn tabs(&self, window: W) -> Option<&[W]> {
        Some(&self.groups[self.find(window)?].members)
    }

    /// The tab in front of `window`'s group: `window` itself if it is in
    /// front, or in no group.
    #[must_use]
    pub fn front(&self, window: W) -> W {
        self.find(window).map_or(window, |index| {
            let group = &self.groups[index];
            group.members[group.front]
        })
    }

    /// Whether `window` should be on screen as far as tabs go: it is in
    /// front of its group, or in none.
    #[must_use]
    pub fn is_front(&self, window: W) -> bool {
        self.front(window) == window
    }

    /// Put `window` into `to`'s group, as the tab after `to`, and in front.
    /// Out of whatever group it was in first. Nothing happens if the two are
    /// the same window.
    pub fn attach(&mut self, window: W, to: W) {
        if window == to {
            return;
        }
        self.detach(window);
        match self.find(to) {
            Some(index) => {
                let group = &mut self.groups[index];
                let at = group
                    .members
                    .iter()
                    .position(|member| *member == to)
                    .map_or(group.members.len(), |at| at + 1);
                group.members.insert(at, window);
                group.front = at;
            }
            None => self.groups.push(Group {
                members: vec![to, window],
                front: 1,
            }),
        }
    }

    /// Take `window` out of its group. Returns the tab now in front of the
    /// group it left, if it was in front of one: that tab takes its place.
    pub fn detach(&mut self, window: W) -> Option<W> {
        let index = self.find(window)?;
        let group = &mut self.groups[index];
        let at = group.members.iter().position(|member| *member == window)?;
        let was_front = group.front == at;
        group.members.remove(at);
        // The tab after it comes forward, or the one before if it was last,
        // which is where a person's eye already is.
        if group.front > at || group.front == group.members.len() {
            group.front = group.front.saturating_sub(1);
        }
        let front = group.members[group.front];
        if group.members.len() < 2 {
            self.groups.remove(index);
        }
        was_front.then_some(front)
    }

    /// Bring `window` to the front of its group.
    pub fn activate(&mut self, window: W) {
        if let Some(index) = self.find(window) {
            let group = &mut self.groups[index];
            if let Some(at) = group.members.iter().position(|member| *member == window) {
                group.front = at;
            }
        }
    }

    /// Bring the next tab forward (or the previous one), wrapping round.
    /// Returns the tab now in front, or `None` if `window` is in no group.
    pub fn cycle(&mut self, window: W, forward: bool) -> Option<W> {
        let index = self.find(window)?;
        let group = &mut self.groups[index];
        let count = group.members.len();
        group.front = if forward {
            (group.front + 1) % count
        } else {
            (group.front + count - 1) % count
        };
        Some(group.members[group.front])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_attached_to_another_makes_a_group_of_two_with_it_in_front() {
        let mut groups = Groups::default();
        groups.attach(2, 1);
        assert_eq!(groups.tabs(1), Some(&[1, 2][..]));
        assert_eq!(groups.front(1), 2);
        assert!(groups.is_front(2));
        assert!(!groups.is_front(1));
        assert!(
            groups.is_front(3),
            "a window in no group is always in front"
        );
    }

    #[test]
    fn a_third_tab_goes_after_the_one_it_joined() {
        let mut groups = Groups::default();
        groups.attach(2, 1);
        groups.attach(3, 1);
        assert_eq!(groups.tabs(2), Some(&[1, 3, 2][..]));
        assert_eq!(groups.front(2), 3);
    }

    #[test]
    fn cycling_wraps_both_ways() {
        let mut groups = Groups::default();
        groups.attach(2, 1);
        groups.attach(3, 2);
        assert_eq!(
            groups.cycle(1, true),
            Some(1),
            "from the last, round to the first"
        );
        assert_eq!(groups.cycle(1, false), Some(3));
        assert_eq!(groups.cycle(9, true), None);
    }

    #[test]
    fn detaching_the_front_tab_brings_its_neighbour_forward() {
        let mut groups = Groups::default();
        groups.attach(2, 1);
        groups.attach(3, 2);
        groups.activate(2);
        assert_eq!(groups.detach(2), Some(3), "the tab after it");
        assert_eq!(groups.tabs(1), Some(&[1, 3][..]));
        assert_eq!(groups.detach(3), Some(1), "the last tab: the one before");
        assert_eq!(groups.tabs(1), None, "and a group of one is no group");
    }

    #[test]
    fn detaching_a_tab_behind_leaves_the_front_one_in_front() {
        let mut groups = Groups::default();
        groups.attach(2, 1);
        groups.attach(3, 2);
        assert_eq!(groups.front(1), 3);
        assert_eq!(groups.detach(1), None);
        assert_eq!(groups.front(2), 3);
        assert_eq!(groups.tabs(2), Some(&[2, 3][..]));
    }

    #[test]
    fn attaching_moves_a_window_out_of_its_old_group() {
        let mut groups = Groups::default();
        groups.attach(2, 1);
        groups.attach(4, 3);
        groups.attach(2, 3);
        assert_eq!(groups.tabs(1), None);
        assert_eq!(groups.tabs(3), Some(&[3, 2, 4][..]));
        groups.attach(1, 1);
        assert_eq!(groups.tabs(1), None, "a window is not its own tab");
    }
}
