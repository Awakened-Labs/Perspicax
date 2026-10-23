//! A client that has stopped reading, under a moving pointer, by a
//! compositor with no screen (#169).
//!
//! A frozen application -- blocked on a D-Bus call to a dead service, a game
//! loading a level, a debugger at a breakpoint -- reads nothing, and what is
//! sent to it waits, up to a megabyte. A pointer moving over it used to fill
//! that, at a motion and a frame a turn, and the client was cut off for it.
//! Now it is owed only where the pointer is: the latest motion, and the
//! mouse's own motion summed, paid before a press, so the press lands where
//! the pointer is, and as soon as it reads again. A leave goes as it is, and
//! forgets what was owed.
//!
//! Each client here falls behind first, by registries, past what the kernel
//! holds for it, so its stall has begun before the pointer moves: what it
//! hears after is exactly what it is sent. A second client, a witness, says
//! when the compositor has done what it was asked (#142). Each holds spare
//! pointers too, every one told each motion, so that a few thousand motions
//! are past a megabyte: each of the compositor's costs a turn of its own
//! work, and 16 000 outlasted the session at nice 15 beside a busy core.
//!
//! The person's mouse is [`Command::Motion`] and [`Command::Click`], which
//! find what is under the pointer from the clients' buffers, so this needs
//! the `capture` feature. Like the other live tests it binds a real Wayland
//! socket, so it needs `XDG_RUNTIME_DIR`, and is `#[ignore]`d for
//! `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use common::{Desk, Pointing, Session, client_over, connect, fall_behind, until};
use perspicax_compositor::{Backend, Command};
use perspicax_index::HostFacts;
use perspicax_node::Rect;
use perspicax_policy::{Button, Mods};
use smithay_client_toolkit::shell::WaylandSurface as _;
use wayland_client::{Connection, EventQueue};

/// The monitor.
const SIZE: (i32, i32) = (800, 600);

/// How far behind a frozen client is before the pointer moves: past what
/// the kernel holds for it, and short of the megabyte it is held to.
const BEHIND: usize = 720_000;

/// How many motions a turn of the loop is given at most: calloop takes up
/// to 1024 of a channel's messages in a dispatch.
const BATCH: usize = 1000;

/// Pointers of a frozen client's own besides the one it hears: with them a
/// motion is 284 bytes, nine motions and frames and the mouse's own.
const SPARE_POINTERS: usize = 8;

/// A window under the pointer, its client frozen, and a witness.
struct Frozen {
    session: Session,
    desk: Desk,
    queue: EventQueue<Desk>,
    /// Kept open: the client's connection.
    _connection: Connection,
    witness: (Desk, EventQueue<Desk>),
    /// The window, in global coordinates.
    window: Rect,
    /// Where the pointer is, in global coordinates.
    at: (i32, i32),
    /// Where the window's origin is, in global coordinates: what is taken
    /// from a point to give it in the window's own.
    origin: (f64, f64),
}

impl Frozen {
    /// A window under the pointer, at its middle, and its client frozen so
    /// far behind that its stall has begun.
    fn new(name: &str) -> Self {
        let session = Session::start(name, Backend::headless(SIZE));
        let connection = connect(session.socket());
        let (mut desk, mut queue, qh, globals) = client_over(&connection);
        desk.bind_pointer(&globals, &qh);
        desk.bind_relative(&globals, &qh);
        desk.bind_spare_pointers(&qh, SPARE_POINTERS);
        desk.open_window(&qh, "frozen", "frozen");
        until(&mut queue, &mut desk, |desk| desk.has_drawn(0));
        // Drawn is not yet described: a motion before the window is in the
        // space would find nothing under the pointer.
        let facts = session.wait_for(|facts| window(facts).is_some());
        let window = window(&facts).expect("described");
        let at = (
            ((window.x0 + window.x1) / 2.0).round() as i32,
            ((window.y0 + window.y1) / 2.0).round() as i32,
        );
        // From the origin, where the pointer starts.
        session.command(Command::Motion { by: at });
        let surface = desk.windows[0].wl_surface().clone();
        until(&mut queue, &mut desk, |desk| {
            desk.pointed.as_ref() == Some(&surface)
        });
        let arrived = desk.arrived.expect("arrived");
        let origin = (f64::from(at.0) - arrived.0, f64::from(at.1) - arrived.1);

        let (witness, witnessed, ..) = session.client();
        desk.pointing.clear();
        fall_behind(&connection, &globals, &qh, BEHIND);
        let mut frozen = Self {
            session,
            desk,
            queue,
            _connection: connection,
            witness: (witness, witnessed),
            window,
            at,
            origin,
        };
        frozen.witnessed();
        frozen
    }

    /// Wait until the compositor has done all it was asked before this:
    /// read every request and command, and flushed what it sent.
    fn witnessed(&mut self) {
        let (witness, queue) = &mut self.witness;
        queue
            .roundtrip(witness)
            .expect("the compositor stopped, so nothing was cut off");
    }

    /// Move the mouse by each of `moves`, a turn's worth at a time.
    fn moved(&mut self, moves: &[(i32, i32)]) {
        for batch in moves.chunks(BATCH) {
            for &by in batch {
                self.session.command(Command::Motion { by });
                self.at = (self.at.0 + by.0, self.at.1 + by.1);
            }
            self.witnessed();
        }
    }

    /// Read all that waits, as a client thawed does: or what the connection
    /// said instead, if it was cut off.
    fn thawed(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut self.desk)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// `at`, in the window's own coordinates.
    fn on_window(&self, at: (i32, i32)) -> (f64, f64) {
        (
            f64::from(at.0) - self.origin.0,
            f64::from(at.1) - self.origin.1,
        )
    }

    fn stop(self) {
        self.session
            .stop(((self.desk, self.queue, self._connection), self.witness));
    }
}

fn window(facts: &HostFacts) -> Option<Rect> {
    facts
        .surfaces()
        .iter()
        .find(|surface| surface.mapped && surface.title.as_deref() == Some("frozen"))
        .map(|surface| surface.geometry)
}

fn inside(window: Rect, at: (i32, i32)) -> bool {
    let (x, y) = (f64::from(at.0), f64::from(at.1));
    window.x0 < x && x < window.x1 && window.y0 < y && y < window.y1
}

/// The mouse back and forth a pixel, `n` times there and back: going
/// nowhere, and summing to nothing.
fn jiggle(n: usize) -> Vec<(i32, i32)> {
    [(1, 0), (-1, 0)].repeat(n)
}

/// 4 000 motions, each 284 bytes to its nine pointers: on top of the 720 KB
/// it is behind, past a megabyte and what the kernel holds, where it was
/// cut off. It is owed one motion instead, and a press brings it, before
/// the press, where the press was.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_frozen_client_under_a_moving_pointer_is_owed_one_motion_not_cut_off() {
    let mut frozen = Frozen::new("frozen-moved-over");
    let mut moves = jiggle(2_000);
    moves.extend([(30, 0), (0, 20)]);
    frozen.moved(&moves);
    let click = (frozen.at.0 - 50, frozen.at.1 - 40);
    assert!(inside(frozen.window, click), "click on the window");
    frozen.session.command(Command::Click {
        at: click,
        button: Button::Left,
        mods: Mods::default(),
    });
    frozen.witnessed();

    assert_eq!(frozen.thawed(), Ok(()), "cut off while it read nothing");
    let left = Button::Left.code();
    assert_eq!(
        frozen.desk.pointing,
        [
            Pointing::Motion(frozen.on_window(click)),
            // Summed: the jiggle comes to nothing, then 30 across, 20 down.
            // The latest alone would be (0, 20).
            Pointing::Relative((30.0, 20.0)),
            Pointing::Frame,
            Pointing::Button(left, true),
            Pointing::Frame,
            Pointing::Button(left, false),
            Pointing::Frame,
        ]
    );
    frozen.stop();
}

/// Owed what it has not been sent, a client reading again is paid it as
/// soon as it does: one motion, where the pointer is, the mouse's own
/// motion summed, and a frame.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_client_that_reads_again_is_paid_the_latest_motion() {
    let mut frozen = Frozen::new("frozen-thawed");
    let mut moves = jiggle(150);
    moves.push((7, 5));
    frozen.moved(&moves);
    let at = frozen.at;

    assert_eq!(frozen.thawed(), Ok(()));
    // The answer to the round trip came before it: the compositor pays
    // only once all that waited has gone.
    until(&mut frozen.queue, &mut frozen.desk, |desk| {
        desk.pointing.contains(&Pointing::Frame)
    });
    assert_eq!(
        frozen.desk.pointing,
        [
            Pointing::Motion(frozen.on_window(at)),
            Pointing::Relative((7.0, 5.0)),
            Pointing::Frame,
        ]
    );
    frozen.stop();
}

/// The pointer leaving a frozen client is told as it is, and what it was
/// owed is forgotten: it never hears of motion on a window it has left.
#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn leaving_a_frozen_client_forgets_what_it_was_owed() {
    let mut frozen = Frozen::new("frozen-left");
    frozen.moved(&jiggle(150));
    let off = (SIZE.0 - 20, SIZE.1 - 20);
    assert!(!inside(frozen.window, off), "off the window");
    frozen.moved(&[(off.0 - frozen.at.0, off.1 - frozen.at.1)]);

    assert_eq!(frozen.thawed(), Ok(()));
    // Time to be paid, were anything owed: what it is sent from now on
    // comes before this round trip's answer.
    frozen.witnessed();
    assert_eq!(frozen.thawed(), Ok(()));
    assert_eq!(frozen.desk.pointing, [Pointing::Leave, Pointing::Frame]);
    frozen.stop();
}
