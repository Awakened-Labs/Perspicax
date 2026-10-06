//! A panel: which monitors have one, the strip of the monitor it takes, and
//! where on it each thing it holds sits.
//!
//! A panel runs the whole width of its monitor, along the top or the
//! bottom. The start button is a square as tall as the panel, the pager as
//! wide as its grid of workspaces, the tray a slot [`TRAY_SLOT`] wide for
//! each icon it shows, and the clock as wide as the time it shows, as is
//! the layout indicator as wide as its label, while there is a choice of
//! layouts to show. The
//! taskbar takes whatever room is left, and shares it among the windows it
//! lists, each no wider than [`TASK_WIDTH`]; a panel without one keeps its
//! last item at the right end, and the rest at the left.

use perspicax_config::{Edge, Item, PanelOutputs};

use super::{Measure, Rect};

/// Room either side of the clock's time.
pub(crate) const CLOCK_PAD: i32 = 10;
/// The widest a task is: a few windows are each this wide, and many share
/// the taskbar.
pub(crate) const TASK_WIDTH: i32 = 200;
/// Room around the pager's grid, and between its cells.
pub(crate) const PAGER_PAD: i32 = 4;
pub(crate) const PAGER_GAP: i32 = 2;
/// How wide each of the tray's icons is, with the room around it.
#[cfg(feature = "tray")]
pub(crate) const TRAY_SLOT: i32 = 30;

/// A window the taskbar lists, as it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Task {
    /// The window's, which stays its own while it is open.
    pub(crate) serial: u64,
    pub(crate) title: String,
    /// Its application's icon, by name.
    pub(crate) icon: Option<String>,
    /// It has the keyboard.
    pub(crate) active: bool,
    pub(crate) minimized: bool,
}

/// A workspace the pager shows, as a cell of its grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cell {
    /// The workspace's, which stays its own while it lasts.
    pub(crate) serial: u64,
    pub(crate) name: String,
    pub(crate) column: u32,
    pub(crate) row: u32,
    /// It is the one showing.
    pub(crate) active: bool,
}

/// A program's status icon in the tray, as it is shown.
#[cfg(feature = "tray")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayIcon {
    /// The icon's, which stays its own while its program shows it.
    pub(crate) key: u64,
    /// What it is called.
    pub(crate) title: String,
    /// It has a menu to open.
    pub(crate) menu: bool,
}

/// What a panel shows beyond its start button: the time, the windows its
/// taskbar lists, its monitor's workspaces, the status icons, and the
/// keyboard layout in use.
pub(crate) struct Holding<'a> {
    pub(crate) time: &'a str,
    /// The layout in use's label, if there is a choice of layouts.
    pub(crate) layout: Option<&'a str>,
    pub(crate) tasks: Vec<Task>,
    pub(crate) cells: Vec<Cell>,
    #[cfg(feature = "tray")]
    pub(crate) tray: Vec<TrayIcon>,
}

/// A panel, laid out: each thing it holds, and where, in its own logical
/// pixels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) items: Vec<(Item, Rect)>,
    pub(crate) tasks: Vec<(Task, Rect)>,
    pub(crate) cells: Vec<(Cell, Rect)>,
    #[cfg(feature = "tray")]
    pub(crate) tray: Vec<(TrayIcon, Rect)>,
}

/// What on a panel a press can be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Start,
    /// The keyboard layout indicator.
    Layout,
    /// The task of the window of this serial.
    Task(u64),
    /// The cell of the workspace of this serial.
    Workspace(u64),
    /// The tray's icon of this key.
    #[cfg(feature = "tray")]
    Tray(u64),
}

/// Which of `monitors`, each a connector name and where its top-left corner
/// is on the desk, have a panel by `rule`, in the order given.
pub(crate) fn chosen<'a>(rule: &PanelOutputs, monitors: &[(&'a str, (i32, i32))]) -> Vec<&'a str> {
    match rule {
        PanelOutputs::All => monitors.iter().map(|&(name, _)| name).collect(),
        PanelOutputs::First => super::first(monitors).into_iter().collect(),
        PanelOutputs::Named(names) => monitors
            .iter()
            .map(|&(name, _)| name)
            .filter(|name| names.iter().any(|named| named == name))
            .collect(),
    }
}

/// The strip of `monitor` a panel `height` high along `edge` takes.
#[cfg_attr(
    not(any(feature = "menus", test)),
    expect(
        dead_code,
        reason = "where menus keep off; no menus, nothing to keep off it"
    )
)]
pub(crate) fn strip(edge: Edge, height: i32, monitor: Rect) -> Rect {
    let height = height.min(monitor.h);
    match edge {
        Edge::Top => Rect::new(monitor.x, monitor.y, monitor.w, height),
        Edge::Bottom => Rect::new(monitor.x, monitor.bottom() - height, monitor.w, height),
    }
}

/// Where each of `items` sits on a panel `size` big, in its own logical
/// pixels, holding what `holding` says.
pub(crate) fn lay_out(
    items: &[Item],
    holding: Holding<'_>,
    (width, height): (i32, i32),
    measure: &mut impl Measure,
) -> Placed {
    let grid = Grid::of(&holding.cells, height);
    let widths: Vec<i32> = items
        .iter()
        .map(|item| match item {
            Item::Start => height,
            Item::Clock => measure.width(holding.time).ceil() as i32 + 2 * CLOCK_PAD,
            Item::Layout => holding.layout.map_or(0, |label| {
                measure.width(label).ceil() as i32 + 2 * CLOCK_PAD
            }),
            Item::Pager => grid.width(),
            // The taskbar's width is what the others leave.
            Item::Taskbar => 0,
            #[cfg(feature = "tray")]
            Item::Tray => holding.tray.len() as i32 * TRAY_SLOT,
            // Refused by the config without a tray to show.
            #[cfg(not(feature = "tray"))]
            Item::Tray => 0,
        })
        .collect();
    let room = (width - widths.iter().sum::<i32>()).max(0);
    let stretch = items
        .iter()
        .position(|&item| item == Item::Taskbar)
        .unwrap_or(items.len().saturating_sub(1));
    let mut x = 0;
    let items: Vec<(Item, Rect)> = items
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(at, (&item, mut w))| {
            if at == stretch {
                match item {
                    Item::Taskbar => w = room,
                    _ => x += room,
                }
            }
            let rect = Rect::new(x, 0, w, height);
            x += w;
            (item, rect)
        })
        .collect();
    let within = |wanted: Item| {
        items
            .iter()
            .find(|&&(item, _)| item == wanted)
            .map(|&(_, rect)| rect)
    };
    let tasks = within(Item::Taskbar).map_or_else(Vec::new, |taskbar| {
        let each = match holding.tasks.len() {
            0 => 0,
            many => (taskbar.w / many as i32).min(TASK_WIDTH),
        };
        holding
            .tasks
            .into_iter()
            .enumerate()
            .map(|(at, task)| {
                let x = taskbar.x + at as i32 * each;
                (task, Rect::new(x, 0, each, height))
            })
            .collect()
    });
    let cells = within(Item::Pager).map_or_else(Vec::new, |pager| {
        holding
            .cells
            .into_iter()
            .map(|cell| {
                let place = grid.cell(pager, &cell);
                (cell, place)
            })
            .collect()
    });
    #[cfg(feature = "tray")]
    let tray = within(Item::Tray).map_or_else(Vec::new, |tray| {
        holding
            .tray
            .into_iter()
            .enumerate()
            .map(|(at, icon)| {
                let x = tray.x + at as i32 * TRAY_SLOT;
                (icon, Rect::new(x, 0, TRAY_SLOT, height))
            })
            .collect()
    });
    Placed {
        items,
        tasks,
        cells,
        #[cfg(feature = "tray")]
        tray,
    }
}

/// The pager's grid: how many columns and rows of cells, each how big.
struct Grid {
    columns: i32,
    rows: i32,
    cell: (i32, i32),
    height: i32,
}

impl Grid {
    /// The grid that holds `cells` on a panel `height` high: each cell the
    /// shape of a monitor, 16 by 10, its rows filling the panel's height.
    fn of(cells: &[Cell], height: i32) -> Self {
        let extent = |at: fn(&Cell) -> u32| cells.iter().map(at).max().map_or(0, |most| most + 1);
        let (columns, rows) = (
            extent(|cell| cell.column) as i32,
            extent(|cell| cell.row) as i32,
        );
        let tall = if rows == 0 {
            0
        } else {
            ((height - 2 * PAGER_PAD - (rows - 1) * PAGER_GAP) / rows).max(1)
        };
        Self {
            columns,
            rows,
            cell: (tall * 8 / 5, tall),
            height,
        }
    }

    /// How wide it is with its margins: nothing, with no cells.
    fn width(&self) -> i32 {
        if self.columns == 0 {
            return 0;
        }
        2 * PAGER_PAD + self.columns * self.cell.0 + (self.columns - 1) * PAGER_GAP
    }

    /// Where `cell` is in the pager at `pager`, its grid in the middle of
    /// the panel's height.
    fn cell(&self, pager: Rect, cell: &Cell) -> Rect {
        let (w, h) = self.cell;
        let tall = self.rows * h + (self.rows - 1).max(0) * PAGER_GAP;
        let top = (self.height - tall) / 2;
        Rect::new(
            pager.x + PAGER_PAD + cell.column as i32 * (w + PAGER_GAP),
            top + cell.row as i32 * (h + PAGER_GAP),
            w,
            h,
        )
    }
}

impl Placed {
    /// What a press at `point` is on, if anything there can be pressed.
    pub(crate) fn at(&self, point: (f64, f64)) -> Option<Part> {
        let task = self
            .tasks
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(task, _)| Part::Task(task.serial));
        let cell = || {
            self.cells
                .iter()
                .find(|(_, rect)| rect.contains(point))
                .map(|(cell, _)| Part::Workspace(cell.serial))
        };
        let item = || {
            self.items.iter().find_map(|&(item, rect)| {
                match item {
                    Item::Start => Some(Part::Start),
                    Item::Layout => Some(Part::Layout),
                    _ => None,
                }
                .filter(|_| rect.contains(point))
            })
        };
        #[cfg(feature = "tray")]
        let tray = || {
            self.tray
                .iter()
                .find(|(_, rect)| rect.contains(point))
                .map(|(icon, _)| Part::Tray(icon.key))
        };
        #[cfg(not(feature = "tray"))]
        let tray = || None;
        task.or_else(cell).or_else(item).or_else(tray)
    }

    /// Where the tray's icon of `key` is, if the panel shows it.
    #[cfg(feature = "tray")]
    pub(crate) fn tray_icon(&self, key: u64) -> Option<Rect> {
        self.tray
            .iter()
            .find(|(icon, _)| icon.key == key)
            .map(|&(_, rect)| rect)
    }

    /// Where `item` is, if the panel holds it.
    #[cfg_attr(
        not(any(feature = "menus", test)),
        expect(
            dead_code,
            reason = "where the start menu stands; no menus, no start menu"
        )
    )]
    pub(crate) fn item(&self, wanted: Item) -> Option<Rect> {
        self.items
            .iter()
            .find(|&&(item, _)| item == wanted)
            .map(|&(_, rect)| rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Monospace;

    const PANEL: (i32, i32) = (1280, 40);

    fn holding(tasks: usize, cells: &[(u32, u32)]) -> Holding<'static> {
        Holding {
            #[cfg(feature = "tray")]
            tray: Vec::new(),
            time: "14:05",
            layout: None,
            tasks: (0..tasks as u64)
                .map(|serial| Task {
                    serial,
                    title: format!("Window {serial}"),
                    icon: None,
                    active: false,
                    minimized: false,
                })
                .collect(),
            cells: cells
                .iter()
                .zip(100..)
                .map(|(&(column, row), serial)| Cell {
                    serial,
                    name: (serial - 99).to_string(),
                    column,
                    row,
                    active: false,
                })
                .collect(),
        }
    }

    fn laid(items: &[Item]) -> Vec<(Item, Rect)> {
        lay_out(items, holding(0, &[]), PANEL, &mut Monospace(8.0)).items
    }

    #[test]
    fn the_start_button_sits_first_and_the_clock_last() {
        let placed = laid(&[Item::Start, Item::Taskbar, Item::Pager, Item::Clock]);
        let clock = 5 * 8 + 2 * CLOCK_PAD;
        assert_eq!(placed[0], (Item::Start, Rect::new(0, 0, 40, 40)), "square");
        assert_eq!(
            placed[1],
            (Item::Taskbar, Rect::new(40, 0, 1280 - 40 - clock, 40)),
            "the room between, with no workspaces to page"
        );
        assert_eq!(
            placed[3],
            (Item::Clock, Rect::new(1280 - clock, 0, clock, 40)),
            "against the right end"
        );
    }

    #[test]
    fn the_layout_indicator_takes_room_only_with_a_choice_of_layouts() {
        let items = [Item::Start, Item::Taskbar, Item::Layout, Item::Clock];
        let one = laid(&items);
        assert_eq!(one[2].1.w, 0, "one layout: nothing to show");
        let others: Vec<_> = one
            .into_iter()
            .filter(|&(item, _)| item != Item::Layout)
            .collect();
        assert_eq!(
            others,
            laid(&[Item::Start, Item::Taskbar, Item::Clock]),
            "and everything else where it was"
        );

        let two = lay_out(
            &items,
            Holding {
                layout: Some("RU"),
                ..holding(0, &[])
            },
            PANEL,
            &mut Monospace(8.0),
        );
        let clock = 5 * 8 + 2 * CLOCK_PAD;
        let label = 2 * 8 + 2 * CLOCK_PAD;
        assert_eq!(
            two.items[2],
            (Item::Layout, Rect::new(1280 - clock - label, 0, label, 40)),
            "as wide as its label, beside the clock"
        );
        assert_eq!(
            two.at((1280.0 - clock as f64 - 10.0, 20.0)),
            Some(Part::Layout)
        );
    }

    #[test]
    fn without_a_taskbar_the_last_item_keeps_the_right_end() {
        let placed = laid(&[Item::Start, Item::Clock]);
        assert_eq!(placed[0].1.x, 0);
        assert_eq!(placed[1].1.right(), 1280);

        let placed = laid(&[Item::Clock, Item::Start]);
        assert_eq!(placed[0].1.x, 0, "the clock first, at the left");
        assert_eq!(placed[1].1, Rect::new(1240, 0, 40, 40));
    }

    #[test]
    fn tasks_share_the_taskbar_each_no_wider_than_a_task() {
        let items = [Item::Start, Item::Taskbar];
        let few = lay_out(&items, holding(2, &[]), PANEL, &mut Monospace(8.0));
        let rects: Vec<Rect> = few.tasks.iter().map(|&(_, rect)| rect).collect();
        assert_eq!(
            rects,
            [
                Rect::new(40, 0, TASK_WIDTH, 40),
                Rect::new(40 + TASK_WIDTH, 0, TASK_WIDTH, 40)
            ],
            "from the taskbar's left end, in order"
        );

        let many = lay_out(&items, holding(16, &[]), PANEL, &mut Monospace(8.0));
        let each = (1280 - 40) / 16;
        assert!(each < TASK_WIDTH);
        assert!(many.tasks.iter().all(|(_, rect)| rect.w == each));
        assert!(
            many.tasks.last().unwrap().1.right() <= 1280,
            "every one in the panel"
        );

        let none = lay_out(&[Item::Start], holding(2, &[]), PANEL, &mut Monospace(8.0));
        assert!(none.tasks.is_empty(), "no taskbar, no tasks");
    }

    #[test]
    fn the_pager_is_a_grid_as_wide_as_its_workspaces() {
        let items = [Item::Taskbar, Item::Pager];
        let row = lay_out(
            &items,
            holding(0, &[(0, 0), (1, 0), (2, 0), (3, 0)]),
            PANEL,
            &mut Monospace(8.0),
        );
        // A row of cells as tall as the panel leaves, 16 by 10.
        let (w, h) = (32 * 8 / 5, 40 - 2 * PAGER_PAD);
        let wide = 2 * PAGER_PAD + 4 * w + 3 * PAGER_GAP;
        assert_eq!(
            row.items[1],
            (Item::Pager, Rect::new(1280 - wide, 0, wide, 40))
        );
        let cells: Vec<Rect> = row.cells.iter().map(|&(_, rect)| rect).collect();
        let left = 1280 - wide + PAGER_PAD;
        assert_eq!(cells[0], Rect::new(left, PAGER_PAD, w, h));
        assert_eq!(cells[3].x, left + 3 * (w + PAGER_GAP));

        let square = lay_out(
            &items,
            holding(0, &[(0, 0), (1, 0), (0, 1), (1, 1)]),
            PANEL,
            &mut Monospace(8.0),
        );
        let cells: Vec<Rect> = square.cells.iter().map(|&(_, rect)| rect).collect();
        assert_eq!(cells[0].y, cells[1].y, "a row");
        assert_eq!(cells[2].x, cells[0].x, "a column");
        assert!(
            cells[2].y > cells[0].bottom(),
            "the second row below the first"
        );
        assert!(cells[3].bottom() <= 40 - PAGER_PAD + 1, "inside the panel");
    }

    #[test]
    fn a_point_finds_what_is_under_it() {
        let items = [Item::Start, Item::Taskbar, Item::Pager, Item::Clock];
        let placed = lay_out(
            &items,
            holding(2, &[(0, 0), (1, 0)]),
            PANEL,
            &mut Monospace(8.0),
        );
        assert_eq!(placed.at((20.0, 20.0)), Some(Part::Start));
        assert_eq!(placed.at((40.0 + 10.0, 20.0)), Some(Part::Task(0)));
        assert_eq!(
            placed.at((40.0 + f64::from(TASK_WIDTH) + 10.0, 20.0)),
            Some(Part::Task(1))
        );
        let second = placed.cells[1].1;
        assert_eq!(
            placed.at((f64::from(second.x + 1), 20.0)),
            Some(Part::Workspace(101))
        );
        assert_eq!(placed.at((1279.0, 1.0)), None, "the clock is not pressed");
        assert_eq!(
            placed.at((600.0, 20.0)),
            None,
            "nor the taskbar's empty end"
        );
        assert_eq!(placed.at((1300.0, 20.0)), None);
        assert_eq!(placed.item(Item::Start), Some(Rect::new(0, 0, 40, 40)));
    }

    #[cfg(feature = "tray")]
    #[test]
    fn the_tray_holds_a_slot_for_each_icon_and_a_press_finds_it() {
        let icon = |key: u64| TrayIcon {
            key,
            title: format!("Icon {key}"),
            menu: true,
        };
        let mut holding = holding(1, &[]);
        holding.tray = vec![icon(7), icon(9)];
        let placed = lay_out(
            &[Item::Start, Item::Taskbar, Item::Tray, Item::Clock],
            holding,
            PANEL,
            &mut Monospace(8.0),
        );
        let clock = 5 * 8 + 2 * CLOCK_PAD;
        let left = 1280 - clock - 2 * TRAY_SLOT;
        assert_eq!(
            placed.items[2],
            (Item::Tray, Rect::new(left, 0, 2 * TRAY_SLOT, 40)),
            "a slot for each, against the clock"
        );
        assert_eq!(
            placed.tray[1].1,
            Rect::new(left + TRAY_SLOT, 0, TRAY_SLOT, 40)
        );
        assert_eq!(
            placed.at((f64::from(left) + 1.0, 20.0)),
            Some(Part::Tray(7))
        );
        assert_eq!(
            placed.at((f64::from(left + 2 * TRAY_SLOT) - 1.0, 20.0)),
            Some(Part::Tray(9))
        );
        assert_eq!(placed.tray_icon(9), Some(placed.tray[1].1));
        assert_eq!(placed.tray_icon(8), None);

        let empty = lay_out(
            &[Item::Start, Item::Tray, Item::Clock],
            self::holding(0, &[]),
            PANEL,
            &mut Monospace(8.0),
        );
        assert_eq!(empty.items[1].1.w, 0, "no icons, no room taken");
    }

    #[test]
    fn outputs_first_puts_one_panel_on_the_first_monitor() {
        let monitors = [
            ("HDMI-A-1", (1920, 0)),
            ("DP-1", (0, 0)),
            ("DP-2", (0, 1080)),
        ];
        assert_eq!(
            chosen(&PanelOutputs::First, &monitors),
            ["DP-1"],
            "the leftmost, and the topmost of those"
        );
        assert_eq!(
            chosen(&PanelOutputs::All, &monitors),
            ["HDMI-A-1", "DP-1", "DP-2"]
        );
        assert_eq!(
            chosen(
                &PanelOutputs::Named(vec!["DP-2".to_owned(), "DP-9".to_owned()]),
                &monitors
            ),
            ["DP-2"],
            "a name with no monitor is no panel"
        );
        assert!(chosen(&PanelOutputs::First, &[]).is_empty());
    }

    #[test]
    fn a_panel_takes_a_strip_along_its_edge() {
        let monitor = Rect::new(0, 0, 1280, 800);
        assert_eq!(
            strip(Edge::Bottom, 40, monitor),
            Rect::new(0, 760, 1280, 40)
        );
        assert_eq!(strip(Edge::Top, 32, monitor), Rect::new(0, 0, 1280, 32));
    }
}
