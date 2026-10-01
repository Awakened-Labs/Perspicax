//! Virtual desktops: a grid of workspaces, and which windows each one shows.
//!
//! Two ways to have more than one monitor, and the config picks one:
//!
//! - **Spanning** (the default): one workspace covers every monitor, and
//!   switching changes all of them together. What Plasma and Windows do, and
//!   what makes a desk of monitors feel like one big screen.
//! - **Per output**: each monitor has its own current workspace and flips on
//!   its own. What Enlightenment does. A window belongs to a cell of the grid,
//!   and which monitor's grid is decided by where the window is, so dragging
//!   a window onto another monitor puts it on that monitor's current cell.
//!
//! This module is the bookkeeping: who is on which cell, which cell each
//! monitor shows, and the arithmetic of moving around the grid. It never says
//! how to hide a window. The compositor asks [`Workspaces::shows`] about every
//! window after each change and maps or unmaps whatever disagrees, so there is
//! one rule for what is on screen and nothing to keep in step with it.

/// A direction on a grid of workspaces, or across a desk of monitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// One workspace, numbered from 0 left to right and then top to bottom. A
/// person sees `index + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cell(pub u16);

/// How workspaces relate to monitors. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Spanning,
    PerOutput,
}

/// The shape of the grid and how moving off its edge behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub columns: u16,
    pub rows: u16,
    /// Moving off one edge comes back on the opposite one.
    pub wrap: bool,
}

impl Default for Grid {
    /// One workspace: what a person who never asked for more gets.
    fn default() -> Self {
        Self {
            columns: 1,
            rows: 1,
            wrap: false,
        }
    }
}

impl Grid {
    /// How many workspaces there are.
    #[must_use]
    pub fn len(&self) -> u16 {
        self.columns.max(1).saturating_mul(self.rows.max(1))
    }

    /// Whether there is only one workspace, and so nowhere to go.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() <= 1
    }

    /// The cell one step from `from`, or `None` at an edge that does not wrap.
    #[must_use]
    pub fn step(&self, from: Cell, direction: Direction) -> Option<Cell> {
        let (columns, rows) = (self.columns.max(1), self.rows.max(1));
        let (column, row) = (from.0 % columns, from.0 / columns);
        let moved = |at: u16, by_one_back: bool, size: u16| -> Option<u16> {
            match (by_one_back, at) {
                (true, 0) if self.wrap => Some(size - 1),
                (true, 0) => None,
                (true, at) => Some(at - 1),
                (false, at) if at + 1 < size => Some(at + 1),
                (false, _) if self.wrap => Some(0),
                (false, _) => None,
            }
        };
        let (column, row) = match direction {
            Direction::Left => (moved(column, true, columns)?, row),
            Direction::Right => (moved(column, false, columns)?, row),
            Direction::Up => (column, moved(row, true, rows)?),
            Direction::Down => (column, moved(row, false, rows)?),
        };
        Some(Cell(row * columns + column))
    }

    /// The workspace after `from` in reading order (left to right, then top
    /// to bottom), or before it when `forward` is false: what a scroll steps
    /// through. `None` past either end of a grid that does not wrap.
    #[must_use]
    pub fn next(&self, from: Cell, forward: bool) -> Option<Cell> {
        let len = self.len();
        match (forward, from.0) {
            (true, at) if at + 1 < len => Some(Cell(at + 1)),
            (true, _) => self.wrap.then_some(Cell(0)),
            (false, 0) => self.wrap.then(|| Cell(len - 1)),
            (false, at) => Some(Cell(at - 1)),
        }
    }

    /// The workspace a person calls `number`, counting from 1, if it exists.
    #[must_use]
    pub fn numbered(&self, number: u16) -> Option<Cell> {
        (1..=self.len()).contains(&number).then(|| Cell(number - 1))
    }
}

/// How many workspaces, in what grid, and how they relate to monitors: what
/// a config's `[workspaces]` table says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shape {
    pub mode: Mode,
    pub grid: Grid,
}

/// Where a window lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    /// On one workspace, shown only while that workspace is.
    On(Cell),
    /// On every workspace: shown whichever one is current.
    Sticky,
}

/// Every window's workspace and every monitor's current one.
///
/// Generic over the window handle `W` and the monitor key `O`, so the
/// compositor uses surface ids and output names and the tests use integers
/// and strings.
#[derive(Debug, Clone)]
pub struct Workspaces<W, O> {
    mode: Mode,
    grid: Grid,
    /// The current workspace in spanning mode, and the one a monitor shows in
    /// per-output mode until it has been switched on its own.
    spanning: Cell,
    /// Per-output mode: what each monitor shows, once it has been switched.
    per_output: Vec<(O, Cell)>,
    windows: Vec<(W, Home)>,
}

impl<W: Copy + PartialEq, O: Clone + PartialEq> Workspaces<W, O> {
    #[must_use]
    pub fn new(Shape { mode, grid }: Shape) -> Self {
        Self {
            mode,
            grid,
            spanning: Cell(0),
            per_output: Vec::new(),
            windows: Vec::new(),
        }
    }

    #[must_use]
    pub fn shape(&self) -> Shape {
        Shape {
            mode: self.mode,
            grid: self.grid,
        }
    }

    /// Take a new mode and grid, as a reloaded config says. Windows on a
    /// workspace that no longer exists collect on the last one, rather than
    /// vanishing with it; a monitor showing one shows the last one instead.
    pub fn reshape(&mut self, Shape { mode, grid }: Shape) {
        let last = Cell(grid.len() - 1);
        let clamp = |cell: &mut Cell| *cell = (*cell).min(last);
        for (_, home) in &mut self.windows {
            if let Home::On(cell) = home {
                clamp(cell);
            }
        }
        clamp(&mut self.spanning);
        for (_, cell) in &mut self.per_output {
            clamp(cell);
        }
        if mode != self.mode {
            // Leaving per-output mode, every monitor follows the spanning
            // workspace; entering it, every monitor starts there.
            self.per_output.clear();
        }
        self.mode = mode;
        self.grid = grid;
    }

    /// The workspace `output` is showing.
    #[must_use]
    pub fn current(&self, output: &O) -> Cell {
        match self.mode {
            Mode::Spanning => self.spanning,
            Mode::PerOutput => self
                .per_output
                .iter()
                .find(|(key, _)| key == output)
                .map_or(self.spanning, |&(_, cell)| cell),
        }
    }

    /// A new window, on the workspace its monitor is showing.
    pub fn add(&mut self, window: W, output: &O) {
        let home = Home::On(self.current(output));
        match self.windows.iter_mut().find(|(known, _)| *known == window) {
            Some((_, known)) => *known = home,
            None => self.windows.push((window, home)),
        }
    }

    /// A window that has closed.
    pub fn remove(&mut self, window: W) {
        self.windows.retain(|(known, _)| *known != window);
    }

    /// Where a window lives, if this has heard of it.
    #[must_use]
    pub fn home(&self, window: W) -> Option<Home> {
        self.windows
            .iter()
            .find(|(known, _)| *known == window)
            .map(|&(_, home)| home)
    }

    /// Whether a window on `output` should be on screen now. A window this
    /// has never heard of is shown: hiding something by omission is the
    /// failure nobody can find.
    #[must_use]
    pub fn shows(&self, window: W, output: &O) -> bool {
        match self.home(window) {
            None | Some(Home::Sticky) => true,
            Some(Home::On(cell)) => cell == self.current(output),
        }
    }

    /// Switch `output` one step across the grid (every monitor, in spanning
    /// mode). `None` at an edge that does not wrap; otherwise the workspace
    /// switched to.
    pub fn switch(&mut self, output: &O, direction: Direction) -> Option<Cell> {
        let to = self.grid.step(self.current(output), direction)?;
        self.go_to(output, to)
    }

    /// Show workspace `to` on `output` (every monitor, in spanning mode).
    /// `None` if there is no such workspace, or it is already showing.
    pub fn go_to(&mut self, output: &O, to: Cell) -> Option<Cell> {
        if to.0 >= self.grid.len() || to == self.current(output) {
            return None;
        }
        match self.mode {
            Mode::Spanning => self.spanning = to,
            Mode::PerOutput => match self.per_output.iter_mut().find(|(key, _)| key == output) {
                Some((_, cell)) => *cell = to,
                None => self.per_output.push((output.clone(), to)),
            },
        }
        Some(to)
    }

    /// Move a window one step across the grid from its workspace, leaving the
    /// person where they are. A sticky window stops being sticky, on the
    /// workspace beside the current one. `None` at an edge that does not wrap.
    pub fn send(&mut self, window: W, output: &O, direction: Direction) -> Option<Cell> {
        let from = match self.home(window)? {
            Home::On(cell) => cell,
            Home::Sticky => self.current(output),
        };
        let to = self.grid.step(from, direction)?;
        self.set(window, Home::On(to));
        Some(to)
    }

    /// Move a window one step across the grid and go with it, so it is still
    /// in front of the person on the workspace they arrive at.
    pub fn carry(&mut self, window: W, output: &O, direction: Direction) -> Option<Cell> {
        let to = self.send(window, output, direction)?;
        self.go_to(output, to);
        Some(to)
    }

    /// Make a window sticky, or put a sticky one back on the current
    /// workspace.
    pub fn toggle_sticky(&mut self, window: W, output: &O) {
        let home = match self.home(window) {
            Some(Home::Sticky) => Home::On(self.current(output)),
            Some(Home::On(_)) => Home::Sticky,
            None => return,
        };
        self.set(window, home);
    }

    /// A window has been moved to another monitor. In per-output mode it
    /// joins the workspace that monitor is showing, so it stays in front of
    /// the person who moved it. In spanning mode nothing changes: every
    /// monitor is showing the same workspace already.
    pub fn moved_to(&mut self, window: W, output: &O) {
        if self.mode == Mode::PerOutput
            && let Some(Home::On(_)) = self.home(window)
        {
            self.set(window, Home::On(self.current(output)));
        }
    }

    /// A monitor is gone. Its per-output workspace is forgotten, so one
    /// plugged in under the same name starts on the default.
    pub fn forget_output(&mut self, output: &O) {
        self.per_output.retain(|(key, _)| key != output);
    }

    fn set(&mut self, window: W, home: Home) {
        if let Some((_, known)) = self.windows.iter_mut().find(|(known, _)| *known == window) {
            *known = home;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_BY_TWO: Grid = Grid {
        columns: 2,
        rows: 2,
        wrap: false,
    };

    fn spanning() -> Workspaces<u32, &'static str> {
        Workspaces::new(Shape {
            mode: Mode::Spanning,
            grid: TWO_BY_TWO,
        })
    }

    fn per_output() -> Workspaces<u32, &'static str> {
        Workspaces::new(Shape {
            mode: Mode::PerOutput,
            grid: TWO_BY_TWO,
        })
    }

    #[test]
    fn steps_across_the_grid_stop_at_the_edges() {
        assert_eq!(TWO_BY_TWO.step(Cell(0), Direction::Right), Some(Cell(1)));
        assert_eq!(TWO_BY_TWO.step(Cell(0), Direction::Down), Some(Cell(2)));
        assert_eq!(TWO_BY_TWO.step(Cell(3), Direction::Up), Some(Cell(1)));
        assert_eq!(TWO_BY_TWO.step(Cell(0), Direction::Left), None);
        assert_eq!(TWO_BY_TWO.step(Cell(3), Direction::Down), None);
    }

    #[test]
    fn a_wrapping_grid_comes_back_on_the_opposite_edge() {
        let grid = Grid {
            wrap: true,
            ..TWO_BY_TWO
        };
        assert_eq!(grid.step(Cell(0), Direction::Left), Some(Cell(1)));
        assert_eq!(grid.step(Cell(2), Direction::Down), Some(Cell(0)));
    }

    #[test]
    fn a_row_of_four_steps_only_sideways() {
        let row = Grid {
            columns: 4,
            rows: 1,
            wrap: false,
        };
        assert_eq!(row.step(Cell(2), Direction::Right), Some(Cell(3)));
        assert_eq!(row.step(Cell(2), Direction::Up), None);
    }

    #[test]
    fn scrolling_reads_through_the_grid_row_by_row() {
        assert_eq!(
            TWO_BY_TWO.next(Cell(1), true),
            Some(Cell(2)),
            "onto the next row"
        );
        assert_eq!(TWO_BY_TWO.next(Cell(3), true), None);
        assert_eq!(TWO_BY_TWO.next(Cell(0), false), None);
        let wrapping = Grid {
            wrap: true,
            ..TWO_BY_TWO
        };
        assert_eq!(wrapping.next(Cell(3), true), Some(Cell(0)));
        assert_eq!(wrapping.next(Cell(0), false), Some(Cell(3)));
    }

    #[test]
    fn workspaces_are_numbered_from_one() {
        assert_eq!(TWO_BY_TWO.numbered(1), Some(Cell(0)));
        assert_eq!(TWO_BY_TWO.numbered(4), Some(Cell(3)));
        assert_eq!(TWO_BY_TWO.numbered(0), None);
        assert_eq!(TWO_BY_TWO.numbered(5), None);
    }

    #[test]
    fn a_spanning_switch_changes_every_monitor() {
        let mut desk = spanning();
        desk.add(1, &"DP-1");
        desk.add(2, &"HDMI-A-1");
        assert_eq!(desk.switch(&"DP-1", Direction::Right), Some(Cell(1)));
        assert_eq!(desk.current(&"HDMI-A-1"), Cell(1), "the other monitor too");
        assert!(!desk.shows(1, &"DP-1"));
        assert!(!desk.shows(2, &"HDMI-A-1"));
    }

    #[test]
    fn a_per_output_switch_changes_one_monitor() {
        let mut desk = per_output();
        desk.add(1, &"DP-1");
        desk.add(2, &"HDMI-A-1");
        desk.switch(&"DP-1", Direction::Down);
        assert_eq!(desk.current(&"DP-1"), Cell(2));
        assert_eq!(desk.current(&"HDMI-A-1"), Cell(0), "untouched");
        assert!(!desk.shows(1, &"DP-1"));
        assert!(desk.shows(2, &"HDMI-A-1"));
    }

    #[test]
    fn a_new_window_opens_on_its_monitors_current_workspace() {
        let mut desk = per_output();
        desk.switch(&"DP-1", Direction::Right);
        desk.add(1, &"DP-1");
        desk.add(2, &"HDMI-A-1");
        assert_eq!(desk.home(1), Some(Home::On(Cell(1))));
        assert_eq!(desk.home(2), Some(Home::On(Cell(0))));
    }

    #[test]
    fn switching_off_the_edge_of_the_grid_does_nothing() {
        let mut desk = spanning();
        assert_eq!(desk.switch(&"DP-1", Direction::Up), None);
        assert_eq!(desk.current(&"DP-1"), Cell(0));
    }

    #[test]
    fn going_to_the_workspace_already_showing_is_not_a_switch() {
        let mut desk = spanning();
        assert_eq!(desk.go_to(&"DP-1", Cell(0)), None);
        assert_eq!(desk.go_to(&"DP-1", Cell(9)), None, "no such workspace");
    }

    #[test]
    fn sending_a_window_leaves_the_person_where_they_are() {
        let mut desk = spanning();
        desk.add(1, &"DP-1");
        assert_eq!(desk.send(1, &"DP-1", Direction::Right), Some(Cell(1)));
        assert_eq!(desk.current(&"DP-1"), Cell(0));
        assert!(!desk.shows(1, &"DP-1"));
    }

    #[test]
    fn carrying_a_window_takes_the_person_with_it() {
        let mut desk = spanning();
        desk.add(1, &"DP-1");
        desk.add(2, &"DP-1");
        assert_eq!(desk.carry(1, &"DP-1", Direction::Down), Some(Cell(2)));
        assert_eq!(desk.current(&"DP-1"), Cell(2));
        assert!(desk.shows(1, &"DP-1"));
        assert!(!desk.shows(2, &"DP-1"), "left behind");
    }

    #[test]
    fn a_sticky_window_is_on_every_workspace_until_unstuck() {
        let mut desk = spanning();
        desk.add(1, &"DP-1");
        desk.toggle_sticky(1, &"DP-1");
        desk.switch(&"DP-1", Direction::Right);
        assert!(desk.shows(1, &"DP-1"));
        desk.toggle_sticky(1, &"DP-1");
        assert_eq!(
            desk.home(1),
            Some(Home::On(Cell(1))),
            "unstuck onto the workspace it was seen on"
        );
    }

    #[test]
    fn a_window_dragged_to_another_monitor_joins_its_workspace_per_output() {
        let mut desk = per_output();
        desk.switch(&"HDMI-A-1", Direction::Right);
        desk.add(1, &"DP-1");
        desk.moved_to(1, &"HDMI-A-1");
        assert_eq!(desk.home(1), Some(Home::On(Cell(1))));
        assert!(desk.shows(1, &"HDMI-A-1"));
    }

    #[test]
    fn moving_a_window_between_monitors_changes_nothing_when_spanning() {
        let mut desk = spanning();
        desk.add(1, &"DP-1");
        desk.moved_to(1, &"HDMI-A-1");
        assert_eq!(desk.home(1), Some(Home::On(Cell(0))));
    }

    #[test]
    fn an_unknown_window_is_shown() {
        let desk = spanning();
        assert!(desk.shows(7, &"DP-1"));
    }

    #[test]
    fn a_smaller_grid_gathers_the_lost_workspaces_windows_on_the_last() {
        let mut desk = spanning();
        desk.add(1, &"DP-1");
        desk.go_to(&"DP-1", Cell(3));
        desk.add(2, &"DP-1");
        desk.reshape(Shape {
            mode: Mode::Spanning,
            grid: Grid {
                columns: 2,
                rows: 1,
                wrap: false,
            },
        });
        assert_eq!(desk.home(2), Some(Home::On(Cell(1))));
        assert_eq!(desk.current(&"DP-1"), Cell(1));
        assert_eq!(desk.home(1), Some(Home::On(Cell(0))), "untouched");
    }
}
