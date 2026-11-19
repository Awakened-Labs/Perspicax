//! The panel's clock: the time as the config's format writes it, and how
//! long until that changes.
//!
//! The format is strftime's, as jiff writes it in the POSIX `C` locale, so
//! `%c`, `%x` and `%r` read as `date` writes them. One jiff cannot write is
//! reported, and the clock shows hours and minutes instead: a typo in the
//! config costs the format, not the clock. A format that shows seconds is
//! redrawn every second; any other on the minute.

use std::time::Duration;

use jiff::{
    Timestamp, Zoned,
    fmt::strtime::{BrokenDownTime, Config, PosixCustom},
    tz::TimeZone,
};

/// What the clock shows when its format cannot be written.
const FALLBACK: &str = "%H:%M";

/// The first and the last instant of one minute. A format writes them
/// alike unless it shows something that changes within a minute.
const MINUTE: [Timestamp; 2] = [
    Timestamp::constant(0, 0),
    Timestamp::constant(59, 999_999_999),
];

/// A clock, its format read.
#[derive(Debug, Clone)]
pub(crate) struct Clock {
    format: String,
    /// How often what it shows changes.
    every: Duration,
}

impl Clock {
    /// A clock showing `format`, or hours and minutes if it cannot be
    /// written.
    pub(crate) fn new(format: &str) -> Self {
        let [first, last] = MINUTE.map(|at| at.to_zoned(TimeZone::UTC));
        let format = if write(format, &first).is_some() {
            format
        } else {
            tracing::warn!("the clock's format {format:?} cannot be read; showing {FALLBACK}");
            FALLBACK
        };
        // Asked of the format itself, so no list of what shows seconds has
        // to keep up with jiff's.
        let every = if write(format, &first) == write(format, &last) {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(1)
        };
        Self {
            format: format.to_owned(),
            every,
        }
    }

    /// The time `now`, as the clock shows it.
    pub(crate) fn show(&self, now: &Zoned) -> String {
        // A format that wrote once writes every time: what writing fails on
        // is a directive jiff does not know, or something the time lacks,
        // and a time in a zone lacks nothing a directive asks for.
        write(&self.format, now).unwrap_or_default()
    }

    /// How long from `now` until what the clock shows next changes: the
    /// start of the next minute, or of the next second.
    pub(crate) fn until_next(&self, now: &Zoned) -> Duration {
        let into_second = Duration::from_nanos(u64::from(now.subsec_nanosecond().unsigned_abs()));
        let into = if self.every.as_secs() >= 60 {
            Duration::from_secs(u64::from(now.second().unsigned_abs())) + into_second
        } else {
            into_second
        };
        self.every.saturating_sub(into)
    }
}

/// `at`, as `format` writes it in the POSIX locale, or `None` if it cannot.
fn write(format: &str, at: &Zoned) -> Option<String> {
    BrokenDownTime::from(at)
        .to_string_with_config(&Config::new().custom(PosixCustom::new()), format)
        .ok()
}

#[cfg(test)]
mod tests {
    use jiff::{civil, tz};

    use super::*;

    /// 3 October 2026, 14:05:07.25, two hours east of Greenwich.
    fn now() -> Zoned {
        civil::date(2026, 10, 3)
            .at(14, 5, 7, 250_000_000)
            .to_zoned(TimeZone::fixed(tz::offset(2)))
            .unwrap()
    }

    #[test]
    fn the_clock_uses_the_configured_format() {
        assert_eq!(Clock::new("%H:%M").show(&now()), "14:05");
        assert_eq!(
            Clock::new("%a %e %b %H:%M").show(&now()),
            "Sat  3 Oct 14:05",
            "the README's"
        );
        assert_eq!(
            Clock::new("%a %e %b, %l:%M %p").show(&now()),
            "Sat  3 Oct,  2:05 PM"
        );
        assert_eq!(
            Clock::new("%H:%M %J").show(&now()),
            "14:05",
            "one it cannot read is hours and minutes"
        );
    }

    /// `%c`, `%x` and `%r` as the `C` locale writes them, as they were
    /// written before the clock kept time with jiff (#319).
    #[test]
    fn the_locales_formats_are_posixs() {
        assert_eq!(Clock::new("%c").show(&now()), "Sat Oct  3 14:05:07 2026");
        assert_eq!(Clock::new("%x").show(&now()), "10/03/26");
        assert_eq!(Clock::new("%r").show(&now()), "02:05:07 PM");
    }

    #[test]
    fn it_ticks_on_the_minute_or_the_second_as_its_format_shows() {
        assert_eq!(
            Clock::new("%H:%M").until_next(&now()),
            Duration::from_millis(52_750),
            "to 14:06:00"
        );
        for seconds in ["%H:%M:%S", "%T", "%c", "%r", "%s", "%N"] {
            assert_eq!(
                Clock::new(seconds).until_next(&now()),
                Duration::from_millis(750),
                "{seconds} shows seconds"
            );
        }
        assert_eq!(
            Clock::new("%A %p %Z %Q").until_next(&now()),
            Duration::from_millis(52_750),
            "names change on the minute at most"
        );
        assert_eq!(
            Clock::new("%H:%M %J").until_next(&now()),
            Duration::from_millis(52_750),
            "one it cannot read ticks as hours and minutes do"
        );
    }
}
