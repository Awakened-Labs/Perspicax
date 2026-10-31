//! Which window has the keyboard, and which is on top.
//!
//! Focus and stacking are separate decisions. Under click-to-focus they
//! always happen together, which makes them easy to confuse, but sloppy focus
//! without autoraise is exactly the case where they come apart: the window
//! under the pointer takes the keyboard and stays where it is in the stack.
//! So every decision here names both parts separately.

use std::time::{Duration, Instant};

use xkeysym::Keysym;

/// How the keyboard follows the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusModel {
    /// A window takes focus when it is clicked. What Plasma and Windows do,
    /// and the default.
    #[default]
    Click,
    /// A window takes focus when the pointer enters it, and keeps it while the
    /// pointer crosses the empty desktop. The usual choice on Fluxbox.
    Sloppy,
    /// A window has focus only while the pointer is over it. Over the empty
    /// desktop, nothing has focus.
    Strict,
}

impl FocusModel {
    /// Every model, in the order the README lists them.
    pub const ALL: [Self; 3] = [Self::Click, Self::Sloppy, Self::Strict];

    /// Its name as `[focus] model` writes it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Sloppy => "sloppy",
            Self::Strict => "strict",
        }
    }
}

/// The focus policy: a model, and whether a window focused by pointing is
/// also raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Focus {
    pub model: FocusModel,
    /// Raise a window when pointing at it focuses it. Has no effect under
    /// [`FocusModel::Click`], where the click raises anyway.
    pub autoraise: bool,
}

/// What happens to keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<W> {
    /// Leave it where it is.
    Keep,
    /// Give it to this window.
    To(W),
    /// Take it away from every window.
    Clear,
}

/// One decision: where the keyboard goes, and what comes to the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision<W> {
    pub focus: Change<W>,
    pub raise: Option<W>,
}

impl<W> Decision<W> {
    /// Nothing changes.
    #[must_use]
    pub fn none() -> Self {
        Self {
            focus: Change::Keep,
            raise: None,
        }
    }
}

impl Focus {
    /// The pointer moved, and is now over `under` (or over no window).
    ///
    /// Called for every motion, not only when the pointer crosses into a new
    /// window, so a window that already has focus decides [`Change::Keep`].
    /// That makes the caller's job "apply what was decided" rather than "work
    /// out whether this was a crossing".
    #[must_use]
    pub fn pointer_over<W: Copy + PartialEq>(
        &self,
        under: Option<W>,
        focused: Option<W>,
    ) -> Decision<W> {
        let entered = under.filter(|&window| Some(window) != focused);
        let focus = match (self.model, under, entered) {
            (FocusModel::Click, _, _) => return Decision::none(),
            (_, _, Some(window)) => Change::To(window),
            (FocusModel::Strict, None, _) if focused.is_some() => Change::Clear,
            _ => Change::Keep,
        };
        let raise = match focus {
            Change::To(window) if self.autoraise => Some(window),
            _ => None,
        };
        Decision { focus, raise }
    }

    /// A button went down over `under` (or over no window).
    ///
    /// Every model raises what is clicked, and every model focuses it. A click
    /// is the one gesture that means "this window" under any policy. A click
    /// on the empty desktop changes nothing: under click-to-focus that is what
    /// Plasma does, and under strict focus the pointer leaving the window has
    /// already cleared it.
    #[must_use]
    pub fn pressed<W: Copy + PartialEq>(
        &self,
        under: Option<W>,
        focused: Option<W>,
    ) -> Decision<W> {
        match under {
            Some(window) => Decision {
                focus: if focused == Some(window) {
                    Change::Keep
                } else {
                    Change::To(window)
                },
                raise: Some(window),
            },
            None => Decision::none(),
        }
    }
}

/// The window a "next window" chord should bring forward, given the stack
/// bottom to top and the window that has the keyboard.
///
/// The bottom one, so pressing the chord repeatedly walks through every
/// window in turn: each press lifts the bottom window to the top, and the
/// next press finds a different one at the bottom. Swapping the top two
/// instead would never reach a third window.
///
/// A lone window is brought forward only if it does not already have the
/// keyboard: a minimized one that is the only window on the workspace comes
/// back, and an unfocused one is focused. Otherwise there is nothing to cycle,
/// and `None`.
#[must_use]
pub fn cycle<W: Copy + PartialEq>(bottom_to_top: &[W], focused: Option<W>) -> Option<W> {
    match bottom_to_top {
        [first, _, ..] => Some(*first),
        [only] if focused != Some(*only) => Some(*only),
        _ => None,
    }
}

/// How old an activation token may be and still move focus.
///
/// Long enough for a slow application to start and map its window after the
/// click that launched it; short enough that a token saved up and spent later
/// cannot pull focus away from whatever the person is doing by then. Sooner
/// than that, a token is refused once the person moves on ([`Attention`]).
pub const ACTIVATION_WINDOW: Duration = Duration::from_secs(10);

/// Whether an `xdg-activation` request may take focus.
///
/// Only a token minted in answer to the person's own input, and only while
/// it is fresh. The compositor says whether input was behind it: for an xdg
/// token, that the client with the keys made it, since a serial any client
/// can name is no proof alone (#187), or the client the person's latest
/// press went to, naming that press, and that the person has not moved on
/// since it was made ([`Attention`], #257); for an X11 request, the X time of
/// the input it carries, believed only while an X11 window has the keys,
/// since any X client can learn the time (#256). Anything else is a window
/// trying to take focus for itself, which is the focus stealing the protocol
/// exists to prevent. A refused activation is not an error. The window simply
/// stays where it is.
#[must_use]
pub fn grants_activation(age: Duration, from_input: bool) -> bool {
    from_input && age < ACTIVATION_WINDOW
}

/// How far the person's attention has come: an activation token is good
/// only on the turn it was made (#257). Stamped on a token as it is made,
/// and compared as it is spent. And the press a program was started by
/// (#259), stamped on its connection as it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn(u64);

/// Where the person's attention has been, for telling an activation token
/// made since from one made before they moved on (#257).
///
/// It turns with every key or button the person presses, and every time the
/// keys go to another client: a window opening, a click, Alt+Tab, the
/// pointer under sloppy focus, a workspace switch, the lock. Except as they
/// leave a launcher, which keeps them while it is up (#131), so that they
/// leave one only as it goes, and the program it started spends its token
/// then. A token made on an earlier turn was made before the person moved
/// on: spent now, it would pull a window forward into whatever they type
/// next, a password at a terminal's `sudo` prompt say. Releases and motion
/// are no turn. Whatever is pressed to open a link or start a program is
/// pressed before its token is made, and a release that moves the keys is
/// counted as they move.
///
/// It also keeps the person's latest press, and which client it reached, or
/// was handed on to by a binding asking the shell for a menu (#267). A
/// notification or a dock never has the keys, and the click it was sent is
/// what vouches for a token it makes, until the next turn.
///
/// And what that press started (#259): a program a binding of perspicax's
/// started with it, or one started since under the client a click or an
/// Enter reached -- a terminal the person ran something in, a menu or a
/// dock they picked from. Such a program's window opens with the keys, as
/// one the person asked for. That holds until they press anything else, the
/// keys handed on meanwhile or not: a splash screen closing hands them back
/// before its program's window opens, and nothing is typed without a press.
#[derive(Debug, Clone)]
pub struct Attention<S, C> {
    turns: u64,
    /// Which client has the keys, and whether as a launcher. `None` while
    /// nothing has them.
    keys: Option<(C, bool)>,
    press: Option<Latest<S, C>>,
}

/// The person's latest press.
#[derive(Debug, Clone)]
struct Latest<S, C> {
    /// What it was sent with, if a client was sent it.
    serial: S,
    at: Instant,
    /// When, in the clock a process's start is told in: ticks since boot.
    boot: u64,
    /// The turn it made. It vouches for a token only while that is the turn.
    turn: Turn,
    /// The client it reached, once it has.
    reached: Option<C>,
    /// What it started programs under, if anything: the process of the
    /// client it reached, or what a binding started with it.
    root: Option<Process>,
}

/// A process, told from any later one given its number (#259): its pid, and
/// when it started, in the kernel's clock ticks since boot. A pid is given
/// again once its process is gone, and the start is what tells the two apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub started: u64,
}

impl<S, C> Default for Attention<S, C> {
    fn default() -> Self {
        Self {
            turns: 0,
            keys: None,
            press: None,
        }
    }
}

impl<S: PartialEq, C: PartialEq> Attention<S, C> {
    /// The turn it is now.
    #[must_use]
    pub fn turn(&self) -> Turn {
        Turn(self.turns)
    }

    /// The person pressed a key or a button at `at`, or `boot` in ticks
    /// since boot, to be sent as `serial` to whatever takes it. Whether a
    /// binding takes it, or a client, it is a turn.
    pub fn pressed(&mut self, serial: S, at: Instant, boot: u64) {
        self.turns += 1;
        self.press = Some(Latest {
            serial,
            at,
            boot,
            turn: self.turn(),
            reached: None,
            root: None,
        });
    }

    /// The keys went to `client`, or to nothing, and to a launcher if
    /// `launcher`. A turn if another client has them now, unless a launcher
    /// had them. One that keeps them is no turn: its menu, a dialog, another
    /// of its windows.
    pub fn keys_to(&mut self, client: Option<C>, launcher: bool) {
        let left_a_launcher = matches!(self.keys, Some((_, true)));
        let kept = self.keys.as_ref().map(|(held, _)| held) == client.as_ref();
        if !kept && !left_a_launcher {
            self.turns += 1;
        }
        self.keys = client.map(|client| (client, launcher));
    }

    /// A press sent as `serial` reached `client`, whose process is `root` if
    /// what it starts now the person started: a click, or an Enter. Remembered
    /// only if it is the person's latest: any other is an agent's, or long
    /// gone.
    pub fn reached(&mut self, serial: &S, client: C, root: Option<Process>) {
        if let Some(press) = &mut self.press
            && press.serial == *serial
        {
            press.reached = Some(client);
            press.root = root;
        }
    }

    /// A binding of perspicax's started `process` with the person's latest
    /// press: a binding takes the press for itself, and no client is sent it.
    pub fn spawned(&mut self, process: Process) {
        if let Some(press) = &mut self.press {
            press.root = Some(process);
        }
    }

    /// A binding of perspicax's handed the person's latest press on to
    /// `client`, asking its shell for a menu or a pie (#267): as though the
    /// press had reached it, starting nothing there. A binding takes the
    /// press for itself, and no client is sent it, so the menu the shell
    /// shows in answer would otherwise be one nobody asked for.
    pub fn handed(&mut self, client: C) {
        if let Some(press) = &mut self.press {
            press.reached = Some(client);
        }
    }

    /// Whether `client` was sent the person's latest press as `serial`, less
    /// than [`ACTIVATION_WINDOW`] before `now`, and nothing has turned since.
    #[must_use]
    pub fn vouches(&self, serial: &S, client: &C, now: Instant) -> bool {
        self.press.as_ref().is_some_and(|press| {
            press.serial == *serial
                && press.turn == self.turn()
                && press.reached.as_ref() == Some(client)
                && fresh(press.at, now)
        })
    }

    /// The press a process was started by, if the person's latest was: the
    /// program a binding started with it, or one started since under what it
    /// reached. `lineage` is the process, then those it descends from.
    ///
    /// Started since, not only under: every key the person types at a
    /// terminal is a press that reaches it, and a program it ran long ago
    /// and left in the background is under it too. Only an Enter or a click
    /// roots anything at all ([`is_enter`]), and what was already running
    /// as it was pressed was not started by it.
    #[must_use]
    pub fn started(&self, lineage: &[Process], now: Instant) -> Option<Turn> {
        let press = self.press.as_ref()?;
        let (own, root) = (lineage.first()?, press.root?);
        (fresh(press.at, now) && own.started >= press.boot && lineage.contains(&root))
            .then_some(press.turn)
    }

    /// Whether a window `client` maps may take the keys from `holder`,
    /// whatever has them, or nothing: whether the person asked for it (#259).
    ///
    /// They did if it is a window of the program they are using, a dialog
    /// say, or if nothing has the keys. Or if their latest press, under
    /// [`ACTIVATION_WINDOW`] old, reached the program -- a dock or a panel
    /// clicked opening a window of its own -- or started it, the press
    /// `started_on` names ([`Self::started`]). Any other program is
    /// opening a window for itself, while the person may be typing in
    /// another: a password at a terminal's `sudo` prompt say.
    ///
    /// A token is the other way a window is asked for, granted as it is
    /// spent, and not asked here.
    #[must_use]
    pub fn asked_for(
        &self,
        client: &C,
        holder: Option<&C>,
        started_on: Option<Turn>,
        now: Instant,
    ) -> bool {
        holder.is_none() || self.asked_of(client, holder, started_on, now)
    }

    /// Whether the person asked something of `client`, whatever has the
    /// keys, `holder`: as a window is asked for, but never for nothing
    /// having them. What a launcher shows -- a layer over the windows that
    /// wants every key -- takes the keys only if it was (#267), what a
    /// taskbar asks of a window is done only if it was (#270), and so is a
    /// switch a pager asks for (#278).
    ///
    /// It was if it has the keys, a run box the terminal in use opened say,
    /// or if their latest press, under [`ACTIVATION_WINDOW`] old, reached it
    /// -- the panel clicked -- or started it, the press `started_on` names
    /// ([`Self::started`]). That holds until they press anything else, the
    /// keys handed on meanwhile or not, as what a press started does: a
    /// task clicked on another workspace moves the keys twice, once as the
    /// workspace shows and once as its window comes forward.
    ///
    /// A window that takes the keys gives them up to the next one the person
    /// clicks. A launcher keeps them from every window while it is up
    /// (#131), so one given them on an empty desk by a program nobody
    /// started would keep them from the terminal the person opened next.
    /// And a program nobody asked that acts on a window acts for itself: it
    /// brings its own window forward, or minimizes the one in use, and the
    /// keys go with it. What the person starts from an empty desk, with a
    /// binding or a click, is asked for all the same.
    #[must_use]
    pub fn asked_of(
        &self,
        client: &C,
        holder: Option<&C>,
        started_on: Option<Turn>,
        now: Instant,
    ) -> bool {
        holder == Some(client)
            || self.press.as_ref().is_some_and(|press| {
                fresh(press.at, now)
                    && (press.reached.as_ref() == Some(client) || started_on == Some(press.turn))
            })
    }
}

/// Whether a press at `at` is under [`ACTIVATION_WINDOW`] old at `now`.
fn fresh(at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(at) < ACTIVATION_WINDOW
}

/// Whether a key is an Enter: the one key whose press starts what the person
/// typed, and so roots what starts under the program it reached (#259). Any
/// other key typed at a terminal -- a password, letter by letter -- starts
/// nothing there, and may not let a program started meanwhile take the keys.
#[must_use]
pub fn is_enter(sym: Keysym) -> bool {
    [Keysym::Return, Keysym::KP_Enter, Keysym::ISO_Enter].contains(&sym)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(model: FocusModel, autoraise: bool) -> Focus {
        Focus { model, autoraise }
    }

    #[test]
    fn click_to_focus_ignores_the_pointer_moving() {
        let decision = Focus::default().pointer_over(Some(2), Some(1));
        assert_eq!(decision, Decision::none());
    }

    #[test]
    fn a_click_focuses_and_raises_under_every_model() {
        for model in [FocusModel::Click, FocusModel::Sloppy, FocusModel::Strict] {
            let decision = policy(model, false).pressed(Some(2), Some(1));
            assert_eq!(decision.focus, Change::To(2), "{model:?}");
            assert_eq!(decision.raise, Some(2), "{model:?}");
        }
    }

    #[test]
    fn clicking_the_focused_window_raises_it_without_refocusing() {
        let decision = Focus::default().pressed(Some(1), Some(1));
        assert_eq!(decision.focus, Change::Keep);
        assert_eq!(decision.raise, Some(1));
    }

    #[test]
    fn clicking_the_empty_desktop_changes_nothing() {
        assert_eq!(
            Focus::default().pressed::<u32>(None, Some(1)),
            Decision::none()
        );
    }

    #[test]
    fn sloppy_focus_follows_the_pointer_into_a_window_without_raising_it() {
        let decision = policy(FocusModel::Sloppy, false).pointer_over(Some(2), Some(1));
        assert_eq!(decision.focus, Change::To(2));
        assert_eq!(decision.raise, None);
    }

    #[test]
    fn sloppy_focus_keeps_the_window_when_the_pointer_crosses_the_desktop() {
        let decision = policy(FocusModel::Sloppy, false).pointer_over(None, Some(1));
        assert_eq!(decision.focus, Change::Keep);
    }

    #[test]
    fn strict_focus_clears_over_the_desktop() {
        let decision = policy(FocusModel::Strict, false).pointer_over(None, Some(1));
        assert_eq!(decision.focus, Change::Clear);
    }

    #[test]
    fn strict_focus_over_the_desktop_with_nothing_focused_is_quiet() {
        let decision = policy(FocusModel::Strict, false).pointer_over::<u32>(None, None);
        assert_eq!(decision, Decision::none());
    }

    #[test]
    fn autoraise_raises_what_pointing_focused() {
        let decision = policy(FocusModel::Sloppy, true).pointer_over(Some(2), Some(1));
        assert_eq!(decision.raise, Some(2));
    }

    #[test]
    fn moving_within_the_focused_window_decides_nothing() {
        let decision = policy(FocusModel::Sloppy, true).pointer_over(Some(1), Some(1));
        assert_eq!(decision, Decision::none());
    }

    #[test]
    fn cycling_brings_the_bottom_window_forward() {
        assert_eq!(cycle(&[1, 2, 3], Some(3)), Some(1));
    }

    #[test]
    fn cycling_the_focused_window_alone_or_none_does_nothing() {
        assert_eq!(cycle(&[1], Some(1)), None);
        assert_eq!(cycle::<u32>(&[], None), None);
    }

    /// Issue #6: the only window on a workspace, minimized or merely
    /// unfocused, is still somewhere Alt+Tab can go.
    #[test]
    fn cycling_brings_back_a_lone_window_that_does_not_have_the_keyboard() {
        assert_eq!(cycle(&[7], None), Some(7), "minimized, or nothing focused");
        assert_eq!(cycle(&[7], Some(9)), Some(7), "focus is somewhere else");
    }

    #[test]
    fn a_fresh_token_from_a_click_may_take_focus() {
        assert!(grants_activation(Duration::from_millis(300), true));
    }

    #[test]
    fn a_token_with_no_input_behind_it_may_not() {
        assert!(!grants_activation(Duration::ZERO, false));
    }

    #[test]
    fn a_stale_token_may_not() {
        assert!(!grants_activation(ACTIVATION_WINDOW, true));
    }

    /// Serials are numbers, clients letters.
    fn attention() -> Attention<u32, char> {
        Attention::default()
    }

    #[test]
    fn every_press_is_a_turn() {
        let mut attention = attention();
        let before = attention.turn();
        attention.pressed(1, Instant::now(), 0);
        let after = attention.turn();
        assert_ne!(before, after);
        attention.pressed(2, Instant::now(), 0);
        assert_ne!(after, attention.turn());
    }

    #[test]
    fn the_keys_going_to_another_client_or_none_are_a_turn() {
        let mut attention = attention();
        let mut turns = vec![attention.turn()];
        attention.keys_to(Some('a'), false);
        turns.push(attention.turn());
        attention.keys_to(Some('b'), false);
        turns.push(attention.turn());
        attention.keys_to(None, false);
        turns.push(attention.turn());
        turns.dedup();
        assert_eq!(turns.len(), 4, "nothing to a, a to b, b to nothing");
    }

    #[test]
    fn the_keys_kept_by_their_client_are_no_turn() {
        let mut attention = attention();
        attention.keys_to(Some('a'), false);
        let turn = attention.turn();
        attention.keys_to(Some('a'), false);
        attention.keys_to(None, false);
        attention.keys_to(None, false);
        assert_eq!(attention.turn().0, turn.0 + 1, "only a to nothing");
    }

    /// Fuzzel goes, the keys go back to the window under it, and the program
    /// it started spends its token.
    #[test]
    fn the_keys_leaving_a_launcher_are_no_turn_but_going_to_one_are() {
        let mut attention = attention();
        attention.keys_to(Some('a'), false);
        let before = attention.turn();
        attention.keys_to(Some('l'), true);
        let up = attention.turn();
        assert_ne!(before, up, "a to the launcher");
        attention.keys_to(Some('a'), false);
        assert_eq!(up, attention.turn(), "the launcher back to a");
        attention.keys_to(Some('l'), true);
        attention.keys_to(None, false);
        assert_ne!(up, attention.turn(), "a to the launcher again");
    }

    /// A click on a notification, which never has the keys.
    #[test]
    fn the_latest_press_vouches_for_the_client_it_reached() {
        let mut attention = attention();
        let at = Instant::now();
        attention.pressed(7, at, 0);
        assert!(!attention.vouches(&7, &'n', at), "reached nobody yet");
        attention.reached(&7, 'n', None);
        assert!(attention.vouches(&7, &'n', at));
        assert!(!attention.vouches(&8, &'n', at), "another serial");
        assert!(!attention.vouches(&7, &'m', at), "another client");
    }

    /// An agent's click, sent with a serial of its own.
    #[test]
    fn a_press_not_the_persons_latest_reaches_nothing() {
        let mut attention = attention();
        let at = Instant::now();
        attention.pressed(7, at, 0);
        attention.reached(&9, 'n', None);
        assert!(!attention.vouches(&9, &'n', at));
        assert!(!attention.vouches(&7, &'n', at));
    }

    #[test]
    fn a_turn_after_the_press_forgets_it() {
        let at = Instant::now();
        let clicked = || {
            let mut attention = attention();
            attention.keys_to(Some('t'), false);
            attention.pressed(7, at, 0);
            attention.reached(&7, 'n', None);
            attention
        };
        let mut pressed_again = clicked();
        pressed_again.pressed(8, at, 0);
        assert!(!pressed_again.vouches(&7, &'n', at), "pressed again");
        let mut moved = clicked();
        moved.keys_to(Some('u'), false);
        assert!(!moved.vouches(&7, &'n', at), "the keys moved on");
        let mut kept = clicked();
        kept.keys_to(Some('t'), false);
        assert!(kept.vouches(&7, &'n', at), "the keys stayed put");
    }

    #[test]
    fn a_press_vouches_for_no_longer_than_the_activation_window() {
        let mut attention = attention();
        let at = Instant::now();
        attention.pressed(7, at, 0);
        attention.reached(&7, 'n', None);
        let just = ACTIVATION_WINDOW - Duration::from_millis(1);
        assert!(attention.vouches(&7, &'n', at + just));
        assert!(!attention.vouches(&7, &'n', at + ACTIVATION_WINDOW));
    }

    /// Processes are pids with a start; the press is at 100 ticks since boot.
    fn process(pid: u32, started: u64) -> Process {
        Process { pid, started }
    }

    const TERMINAL: Process = Process {
        pid: 10,
        started: 5,
    };

    /// The person pressed 7 at 100 ticks since boot.
    fn pressed_at(at: Instant) -> Attention<u32, char> {
        let mut attention = attention();
        attention.keys_to(Some('t'), false);
        attention.pressed(7, at, 100);
        attention
    }

    /// Logo+Return, say: what the binding started, and what that starts.
    #[test]
    fn a_program_a_binding_started_was_started_by_the_press() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        let started = process(50, 101);
        attention.spawned(started);
        let turn = Some(attention.turn());
        assert_eq!(attention.started(&[started], at), turn);
        assert_eq!(attention.started(&[process(60, 102), started], at), turn);
    }

    /// Enter in a terminal: the program it ran, under its shell.
    #[test]
    fn a_program_started_since_under_the_client_the_press_reached_was_started_by_it() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        let lineage = [process(70, 103), process(65, 50), TERMINAL];
        assert_eq!(attention.started(&lineage, at), Some(attention.turn()));
    }

    /// A job the terminal ran long ago, still in the background.
    #[test]
    fn a_program_already_running_as_the_press_came_was_not_started_by_it() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        assert_eq!(attention.started(&[process(80, 99), TERMINAL], at), None);
    }

    /// A start is told in whole ticks, and a press is floored to one too.
    #[test]
    fn a_program_started_in_the_presss_own_tick_was_started_by_it() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        assert!(
            attention
                .started(&[process(70, 100), TERMINAL], at)
                .is_some()
        );
    }

    #[test]
    fn a_process_given_the_pid_of_the_root_since_is_not_under_it() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        let given_again = process(TERMINAL.pid, 99);
        assert_eq!(
            attention.started(&[process(70, 103), given_again], at),
            None
        );
    }

    /// A letter typed, or a press that reached a client to start nothing.
    #[test]
    fn a_press_with_no_root_starts_nothing() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        assert_eq!(attention.started(&[process(70, 103), TERMINAL], at), None);
        attention.reached(&7, 't', None);
        assert_eq!(attention.started(&[process(70, 103), TERMINAL], at), None);
    }

    /// An agent's click, sent with a serial of its own.
    #[test]
    fn a_press_not_the_persons_latest_roots_nothing() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&9, 't', Some(TERMINAL));
        assert_eq!(attention.started(&[process(70, 103), TERMINAL], at), None);
    }

    #[test]
    fn a_press_starts_nothing_once_the_activation_window_is_past() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        let lineage = [process(70, 103), TERMINAL];
        let just = ACTIVATION_WINDOW - Duration::from_millis(1);
        assert!(attention.started(&lineage, at + just).is_some());
        assert_eq!(attention.started(&lineage, at + ACTIVATION_WINDOW), None);
        let stamp = attention.started(&lineage, at);
        assert!(attention.asked_for(&'p', Some(&'t'), stamp, at + just));
        assert!(!attention.asked_for(&'p', Some(&'t'), stamp, at + ACTIVATION_WINDOW));
    }

    #[test]
    fn a_window_opens_with_the_keys_where_nothing_has_them() {
        assert!(attention().asked_for(&'a', None, None, Instant::now()));
    }

    /// A dialog, or another window of the program in use.
    #[test]
    fn a_window_of_the_program_with_the_keys_opens_with_them() {
        assert!(attention().asked_for(&'a', Some(&'a'), None, Instant::now()));
    }

    /// The issue's own case (#259): a program in the background opens a
    /// window while the person types at a terminal.
    #[test]
    fn a_window_of_another_program_opens_without_them() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        assert!(!attention.asked_for(&'b', Some(&'t'), None, at));
        attention.reached(&7, 't', Some(TERMINAL));
        assert!(!attention.asked_for(&'b', Some(&'t'), None, at));
    }

    /// A dock clicked, opening a window of its own.
    #[test]
    fn a_window_of_the_program_the_latest_press_reached_opens_with_them() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 'd', None);
        assert!(attention.asked_for(&'d', Some(&'t'), None, at));
        attention.pressed(8, at, 101);
        assert!(
            !attention.asked_for(&'d', Some(&'t'), None, at),
            "pressed since"
        );
    }

    #[test]
    fn a_window_of_a_program_the_latest_press_started_opens_with_them() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        let stamp = attention.started(&[process(70, 103), TERMINAL], at);
        assert!(attention.asked_for(&'p', Some(&'t'), stamp, at));
        attention.pressed(8, at, 104);
        assert!(
            !attention.asked_for(&'p', Some(&'t'), stamp, at),
            "pressed since"
        );
    }

    /// Splash screens: the one that took the keys closes, and they go back
    /// to the window under it before the main window opens.
    #[test]
    fn the_keys_handed_on_since_keep_what_a_press_started_but_not_its_vouching() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        let lineage = [process(70, 103), TERMINAL];
        let stamp = attention.started(&lineage, at);
        attention.keys_to(Some('p'), false);
        attention.keys_to(Some('t'), false);
        assert_eq!(attention.started(&lineage, at), stamp);
        assert!(attention.asked_for(&'p', Some(&'t'), stamp, at));
        assert!(!attention.vouches(&7, &'t', at), "no token rides on it");
    }

    /// A run box the terminal in use opened, say (#267).
    #[test]
    fn a_launcher_of_the_program_with_the_keys_may_take_them() {
        assert!(attention().asked_of(&'a', Some(&'a'), None, Instant::now()));
    }

    /// Where a window may: a launcher keeps the keys from every window
    /// while it is up (#131).
    #[test]
    fn a_launcher_shown_where_nothing_has_the_keys_may_not_take_them() {
        let attention = attention();
        let now = Instant::now();
        assert!(!attention.asked_of(&'l', None, None, now));
        assert!(attention.asked_for(&'l', None, None, now), "a window may");
    }

    /// The issue's own case: a program in the background shows a launcher
    /// while the person types at a terminal.
    #[test]
    fn a_launcher_of_another_program_may_not_take_the_keys() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        assert!(!attention.asked_of(&'l', Some(&'t'), None, at));
    }

    /// The shell's panel clicked, opening its menu.
    #[test]
    fn a_launcher_of_the_program_the_latest_press_reached_may_take_the_keys() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 's', None);
        assert!(attention.asked_of(&'s', Some(&'t'), None, at));
        attention.pressed(8, at, 101);
        assert!(
            !attention.asked_of(&'s', Some(&'t'), None, at),
            "pressed since"
        );
    }

    /// fuzzel, from a binding or an Enter at a terminal.
    #[test]
    fn a_launcher_of_a_program_the_latest_press_started_may_take_the_keys() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 't', Some(TERMINAL));
        let stamp = attention.started(&[process(70, 103), TERMINAL], at);
        assert!(attention.asked_of(&'f', Some(&'t'), stamp, at));
        let just = ACTIVATION_WINDOW - Duration::from_millis(1);
        assert!(attention.asked_of(&'f', Some(&'t'), stamp, at + just));
        assert!(
            !attention.asked_of(&'f', Some(&'t'), stamp, at + ACTIVATION_WINDOW),
            "too long ago"
        );
    }

    /// Logo tapped, and the shell asked for its start menu.
    #[test]
    fn a_press_a_binding_handed_on_reaches_the_client_it_was_handed_to() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        assert!(!attention.asked_of(&'s', Some(&'t'), None, at));
        attention.handed('s');
        assert!(attention.asked_of(&'s', Some(&'t'), None, at));
        assert!(
            attention
                .started(&[process(70, 103), TERMINAL], at)
                .is_none(),
            "and starts nothing"
        );
        attention.pressed(8, at, 101);
        assert!(
            !attention.asked_of(&'s', Some(&'t'), None, at),
            "pressed since"
        );
    }

    /// A task clicked on the panel, and its window brought forward (#270):
    /// the keys move, and the panel may still finish what the click asked.
    #[test]
    fn what_a_press_asked_of_a_client_lasts_while_the_keys_move_until_the_next_press() {
        let at = Instant::now();
        let mut attention = pressed_at(at);
        attention.reached(&7, 's', None);
        attention.keys_to(Some('w'), false);
        attention.keys_to(Some('v'), false);
        assert!(attention.asked_of(&'s', Some(&'v'), None, at));
        attention.pressed(8, at, 101);
        assert!(
            !attention.asked_of(&'s', Some(&'v'), None, at),
            "pressed since"
        );
    }

    /// The shell asked for a menu by a test, with nothing pressed.
    #[test]
    fn handing_on_with_no_press_reaches_nothing() {
        let mut attention = attention();
        attention.handed('s');
        assert!(!attention.asked_of(&'s', Some(&'t'), None, Instant::now()));
    }

    #[test]
    fn only_an_enter_starts_what_was_typed() {
        assert!(is_enter(Keysym::Return));
        assert!(is_enter(Keysym::KP_Enter));
        assert!(!is_enter(Keysym::a));
        assert!(!is_enter(Keysym::space));
    }
}
