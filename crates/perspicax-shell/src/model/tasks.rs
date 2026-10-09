//! The windows, as a taskbar is told them, and what a press on one asks of
//! it.
//!
//! The compositor tells of each window one fact at a time: its title, its
//! app id, the monitors it is on, whether it has the keyboard, whether it
//! is minimized, whether it belongs to another window. The facts told
//! since its last `done` take effect together at the next, so a window is
//! never shown half-changed; it is listed from its first `done` until it is
//! `closed`, in the order the windows opened. A window that belongs to
//! another, as a dialog does, is not listed: it comes forward with the
//! window it belongs to.
//!
//! Generic over the handle the compositor gave each window and the monitors
//! it names, so that the shell keeps its Wayland objects here and a test
//! keeps numbers and names.

use perspicax_config::TaskbarScope;

use super::Button;

/// Every window the compositor has told of, in the order they opened.
#[derive(Debug)]
pub(crate) struct Tasks<H, O> {
    each: Vec<Toplevel<H, O>>,
    /// The serial the next window is given.
    next: u64,
}

/// One window, by the handle the compositor gave it.
#[derive(Debug)]
struct Toplevel<H, O> {
    handle: H,
    /// Its own for as long as it is open, and never another's.
    serial: u64,
    /// As of its last `done`; `None` before its first.
    now: Option<Window<O>>,
    /// As told since.
    pending: Window<O>,
}

/// A window, as a taskbar lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Window<O> {
    pub(crate) title: String,
    pub(crate) app_id: String,
    /// The monitors it is on.
    pub(crate) outputs: Vec<O>,
    /// It has the keyboard.
    pub(crate) active: bool,
    pub(crate) minimized: bool,
    /// It belongs to another window.
    pub(crate) child: bool,
}

/// One fact the compositor told of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Told<O> {
    Title(String),
    AppId(String),
    /// It is on this monitor now.
    OutputEnter(O),
    /// It is no longer on this monitor.
    OutputLeave(O),
    /// Whether it has the keyboard, and whether it is minimized.
    State {
        active: bool,
        minimized: bool,
    },
    /// Whether it belongs to another window.
    Parent(bool),
    /// What was told since the last `Done` holds from now.
    Done,
    /// It is gone.
    Closed,
}

/// What a press on a task asks of its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ask {
    /// Bring it forward: show it, minimized or not, on whichever workspace
    /// it is, and give it the keyboard.
    Activate,
    /// Put it away.
    Minimize,
    /// Ask it to close, which it may decline.
    Close,
}

impl<H, O> Default for Tasks<H, O> {
    fn default() -> Self {
        Self {
            each: Vec::new(),
            next: 0,
        }
    }
}

impl<O> Default for Window<O> {
    fn default() -> Self {
        Self {
            title: String::new(),
            app_id: String::new(),
            outputs: Vec::new(),
            active: false,
            minimized: false,
            child: false,
        }
    }
}

impl<H: PartialEq, O: Clone + PartialEq> Tasks<H, O> {
    /// A window the compositor has just told of, of which nothing more is
    /// known yet.
    pub(crate) fn add(&mut self, handle: H) {
        self.each.push(Toplevel {
            handle,
            serial: self.next,
            now: None,
            pending: Window::default(),
        });
        self.next += 1;
    }

    /// Take in what the compositor `told` of the window `handle` is. Whether
    /// that changed what is listed.
    pub(crate) fn tell(&mut self, handle: &H, told: Told<O>) -> bool {
        let Some(at) = self.each.iter().position(|known| known.handle == *handle) else {
            return false;
        };
        let toplevel = &mut self.each[at];
        let pending = &mut toplevel.pending;
        match told {
            Told::Title(title) => pending.title = title,
            Told::AppId(app_id) => pending.app_id = app_id,
            Told::OutputEnter(output) => {
                if !pending.outputs.contains(&output) {
                    pending.outputs.push(output);
                }
            }
            Told::OutputLeave(output) => pending.outputs.retain(|on| *on != output),
            Told::State { active, minimized } => {
                pending.active = active;
                pending.minimized = minimized;
            }
            Told::Parent(child) => pending.child = child,
            Told::Done => {
                let changed = toplevel.now.as_ref() != Some(&toplevel.pending);
                toplevel.now = Some(toplevel.pending.clone());
                return changed;
            }
            Told::Closed => {
                let listed = toplevel.now.is_some();
                self.each.remove(at);
                return listed;
            }
        }
        false
    }

    /// The monitor `output` is gone, and no window is on it any more. Whether
    /// that changed what is listed.
    pub(crate) fn gone(&mut self, output: &O) -> bool {
        let mut changed = false;
        for toplevel in &mut self.each {
            toplevel.pending.outputs.retain(|on| on != output);
            if let Some(now) = &mut toplevel.now {
                let before = now.outputs.len();
                now.outputs.retain(|on| on != output);
                changed |= now.outputs.len() != before;
            }
        }
        changed
    }

    /// Forget every window, as when the compositor stops telling of them.
    /// Whether any was listed.
    pub(crate) fn clear(&mut self) -> bool {
        let listed = self.each.iter().any(|toplevel| toplevel.now.is_some());
        self.each.clear();
        listed
    }

    /// The windows a panel on monitor `output` lists, by serial, in the
    /// order they opened: those on that monitor, or with `scope` all, every
    /// window.
    pub(crate) fn listed(
        &self,
        output: &O,
        scope: TaskbarScope,
    ) -> impl Iterator<Item = (u64, &Window<O>)> {
        self.each.iter().filter_map(move |toplevel| {
            let window = toplevel.now.as_ref()?;
            let here = match scope {
                TaskbarScope::All => true,
                TaskbarScope::ThisOutput => window.outputs.contains(output),
            };
            (here && !window.child).then_some((toplevel.serial, window))
        })
    }

    /// Every window that does not belong to another, by serial, in the
    /// order they opened, wherever it is: what a pie lists.
    #[cfg(feature = "pie")]
    pub(crate) fn all(&self) -> impl Iterator<Item = (u64, &Window<O>)> {
        self.each.iter().filter_map(|toplevel| {
            let window = toplevel.now.as_ref()?;
            (!window.child).then_some((toplevel.serial, window))
        })
    }

    /// The handle to ask window `serial` things by, while it is open.
    #[cfg(feature = "pie")]
    pub(crate) fn handle(&self, serial: u64) -> Option<&H> {
        self.each
            .iter()
            .find(|known| known.serial == serial && known.now.is_some())
            .map(|known| &known.handle)
    }

    /// Every window shown so far.
    pub(crate) fn windows(&self) -> impl Iterator<Item = &Window<O>> {
        self.each
            .iter()
            .filter_map(|toplevel| toplevel.now.as_ref())
    }

    /// What pressing `button` on the task of window `serial` asks of it,
    /// with the handle to ask it by. A left click brings a window forward,
    /// or puts it away if it is forward already; a middle click closes it.
    pub(crate) fn pressed(&self, serial: u64, button: Button) -> Option<(&H, Ask)> {
        let toplevel = self.each.iter().find(|known| known.serial == serial)?;
        let window = toplevel.now.as_ref()?;
        let ask = match button {
            Button::Left if window.active && !window.minimized => Ask::Minimize,
            Button::Left => Ask::Activate,
            Button::Middle => Ask::Close,
            Button::Right | Button::Other => return None,
        };
        Some((&toplevel.handle, ask))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows by number, on monitors by name.
    type Windows = Tasks<u32, &'static str>;

    /// A window told whole: `handle`, titled `title`, on `output`.
    fn opened(tasks: &mut Windows, handle: u32, title: &str, output: &'static str) {
        tasks.add(handle);
        tasks.tell(&handle, Told::Title(title.to_owned()));
        tasks.tell(&handle, Told::AppId(title.to_lowercase()));
        tasks.tell(&handle, Told::OutputEnter(output));
        tasks.tell(&handle, Told::Done);
    }

    fn titles(tasks: &Windows, output: &'static str, scope: TaskbarScope) -> Vec<String> {
        tasks
            .listed(&output, scope)
            .map(|(_, window)| window.title.clone())
            .collect()
    }

    /// The serial of the window titled `title`.
    fn serial(tasks: &Windows, title: &str) -> u64 {
        tasks
            .listed(&"", TaskbarScope::All)
            .find(|(_, window)| window.title == title)
            .expect("listed")
            .0
    }

    #[test]
    fn a_task_appears_on_done_and_goes_on_closed() {
        let mut tasks = Windows::default();
        tasks.add(7);
        assert!(!tasks.tell(&7, Told::Title("Mail".to_owned())));
        assert!(!tasks.tell(&7, Told::OutputEnter("DP-1")));
        assert!(
            titles(&tasks, "DP-1", TaskbarScope::All).is_empty(),
            "nothing listed before its done"
        );
        assert!(tasks.tell(&7, Told::Done), "listed now");
        assert_eq!(titles(&tasks, "DP-1", TaskbarScope::All), ["Mail"]);

        tasks.tell(&7, Told::Title("Mail (1)".to_owned()));
        assert_eq!(
            titles(&tasks, "DP-1", TaskbarScope::All),
            ["Mail"],
            "a new title waits for its done"
        );
        assert!(tasks.tell(&7, Told::Done));
        assert!(!tasks.tell(&7, Told::Done), "nothing new told");
        assert_eq!(titles(&tasks, "DP-1", TaskbarScope::All), ["Mail (1)"]);

        assert!(tasks.tell(&7, Told::Closed));
        assert!(titles(&tasks, "DP-1", TaskbarScope::All).is_empty());
        assert!(!tasks.tell(&7, Told::Done), "a closed window is forgotten");

        tasks.add(8);
        assert!(
            !tasks.tell(&8, Told::Closed),
            "one closed before its done was never listed"
        );
    }

    #[test]
    fn a_panel_lists_only_its_monitors_windows_unless_taskbar_is_all() {
        let mut tasks = Windows::default();
        opened(&mut tasks, 1, "Editor", "DP-1");
        opened(&mut tasks, 2, "Browser", "HDMI-A-1");
        opened(&mut tasks, 3, "Mail", "DP-1");
        assert_eq!(
            titles(&tasks, "DP-1", TaskbarScope::ThisOutput),
            ["Editor", "Mail"],
            "in the order they opened"
        );
        assert_eq!(
            titles(&tasks, "HDMI-A-1", TaskbarScope::ThisOutput),
            ["Browser"]
        );
        assert_eq!(
            titles(&tasks, "HDMI-A-1", TaskbarScope::All),
            ["Editor", "Browser", "Mail"]
        );

        // The browser dragged to the first monitor.
        tasks.tell(&2, Told::OutputLeave("HDMI-A-1"));
        tasks.tell(&2, Told::OutputEnter("DP-1"));
        tasks.tell(&2, Told::Done);
        assert_eq!(
            titles(&tasks, "DP-1", TaskbarScope::ThisOutput),
            ["Editor", "Browser", "Mail"]
        );
        assert!(titles(&tasks, "HDMI-A-1", TaskbarScope::ThisOutput).is_empty());

        // The first monitor unplugged, before the compositor moves anything.
        assert!(tasks.gone(&"DP-1"));
        assert!(titles(&tasks, "DP-1", TaskbarScope::ThisOutput).is_empty());
        assert!(!tasks.gone(&"DP-1"), "nothing left on it");
    }

    #[test]
    fn a_window_that_belongs_to_another_is_not_listed() {
        let mut tasks = Windows::default();
        opened(&mut tasks, 1, "Editor", "DP-1");
        tasks.add(2);
        tasks.tell(&2, Told::Title("Save As".to_owned()));
        tasks.tell(&2, Told::OutputEnter("DP-1"));
        tasks.tell(&2, Told::Parent(true));
        tasks.tell(&2, Told::Done);
        assert_eq!(titles(&tasks, "DP-1", TaskbarScope::All), ["Editor"]);
        assert_eq!(tasks.windows().count(), 2, "though it is known");
    }

    #[test]
    fn clicking_the_active_task_minimizes_it() {
        let mut tasks = Windows::default();
        opened(&mut tasks, 1, "Editor", "DP-1");
        opened(&mut tasks, 2, "Mail", "DP-1");
        let state = |active, minimized| Told::State { active, minimized };
        tasks.tell(&2, state(true, false));
        tasks.tell(&2, Told::Done);

        let (editor, mail) = (serial(&tasks, "Editor"), serial(&tasks, "Mail"));
        assert_eq!(
            tasks.pressed(mail, Button::Left),
            Some((&2, Ask::Minimize)),
            "forward already: put away"
        );
        assert_eq!(
            tasks.pressed(editor, Button::Left),
            Some((&1, Ask::Activate)),
            "behind: brought forward"
        );

        tasks.tell(&2, state(false, true));
        tasks.tell(&2, Told::Done);
        assert_eq!(
            tasks.pressed(mail, Button::Left),
            Some((&2, Ask::Activate)),
            "minimized: brought back"
        );
        assert_eq!(tasks.pressed(99, Button::Left), None, "no such task");
    }

    #[test]
    fn middle_click_closes() {
        let mut tasks = Windows::default();
        opened(&mut tasks, 1, "Editor", "DP-1");
        let editor = serial(&tasks, "Editor");
        assert_eq!(
            tasks.pressed(editor, Button::Middle),
            Some((&1, Ask::Close))
        );
        assert_eq!(tasks.pressed(editor, Button::Right), None);
    }

    #[test]
    fn a_serial_is_never_given_twice() {
        let mut tasks = Windows::default();
        opened(&mut tasks, 1, "Editor", "DP-1");
        let first = serial(&tasks, "Editor");
        tasks.tell(&1, Told::Closed);
        opened(&mut tasks, 1, "Editor", "DP-1");
        assert_ne!(
            serial(&tasks, "Editor"),
            first,
            "a handle used again is a new window"
        );
        assert!(tasks.clear());
        assert_eq!(tasks.windows().count(), 0);
    }
}
