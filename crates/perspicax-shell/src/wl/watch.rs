//! How long one thing keeps the loop.
//!
//! The shell reads what the compositor sends only between the things it
//! does, and wayland-backend cuts off a client that stops reading while it
//! is sent things: a pointer moved over a pie fills what the kernel holds
//! for the shell in seconds. So whatever keeps the loop a quarter of a second
//! or more says so in the shell's log, naming itself and how long it took:
//! the only word on why, should the shell be cut off after it.
//!
//! Each of the loop's sources is timed, and so is what is slow enough to
//! name inside the busiest of them, the compositor's: opening a menu or a
//! pie, drawing one, taking up a changed config. One stall can say so twice,
//! the inner line naming what it was.

use std::time::{Duration, Instant};

/// Long enough to say so: seventy-five frames at 300 Hz, and many times what
/// anything the shell does should take.
const SLOW: Duration = Duration::from_millis(250);

/// Do `work`, and say so if it kept the loop [`SLOW`] or longer.
pub(super) fn timed<T>(what: &str, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let done = work();
    slow(what, started.elapsed());
    done
}

/// Say that `what` kept the loop for `took`, if that was [`SLOW`] or longer.
pub(super) fn slow(what: &str, took: Duration) {
    if took >= SLOW {
        tracing::warn!(
            ?took,
            "the shell read nothing from the compositor while {what}"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use super::*;

    /// What was logged, at warnings and above, while `run` ran.
    fn logged(run: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);

        impl io::Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0
                    .lock()
                    .expect("not poisoned")
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let lines = Lines::default();
        let writer = lines.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let bytes = lines.0.lock().expect("not poisoned").clone();
        String::from_utf8(bytes).expect("text")
    }

    #[test]
    fn work_that_keeps_the_loop_a_quarter_second_is_named_with_how_long_it_took() {
        let said = logged(|| slow("drawing the pie", Duration::from_millis(1500)));
        assert!(said.contains("WARN"), "a warning: {said}");
        assert!(said.contains("drawing the pie"), "named: {said}");
        assert!(said.contains("took=1.5s"), "with how long: {said}");

        let said = logged(|| slow("opening a pie", SLOW));
        assert!(
            said.contains("opening a pie"),
            "from a quarter second: {said}"
        );
    }

    #[test]
    fn quicker_work_says_nothing() {
        let said = logged(|| slow("drawing the pie", SLOW - Duration::from_millis(1)));
        assert_eq!(said, "");
        let said = logged(|| timed("drawing the pie", || ()));
        assert_eq!(said, "");
    }

    #[test]
    fn timed_work_is_timed_whole_and_gives_back_what_it_made() {
        let mut made = 0;
        let said = logged(|| {
            made = timed("opening a pie", || {
                std::thread::sleep(SLOW);
                7
            });
        });
        assert_eq!(made, 7);
        assert!(said.contains("opening a pie"), "slow, so named: {said}");
    }
}
