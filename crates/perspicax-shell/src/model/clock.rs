//! The panel's clock: the time as the config's format writes it, and how
//! long until that changes.
//!
//! The format is strftime's, as chrono reads it. One chrono cannot read is
//! reported, and the clock shows hours and minutes instead: a typo in the
//! config costs the format, not the clock. A format that shows seconds is
//! redrawn every second; any other on the minute.

use std::{fmt::Write as _, time::Duration};

use chrono::{
    DateTime, TimeZone, Timelike,
    format::{Fixed, Item, Numeric, StrftimeItems},
};

/// What the clock shows when its format cannot be read.
const FALLBACK: &str = "%H:%M";

/// A clock, its format read.
#[derive(Debug, Clone)]
pub(crate) struct Clock {
    items: Vec<Item<'static>>,
    /// How often what it shows changes.
    every: Duration,
}

impl Clock {
    /// A clock showing `format`, or hours and minutes if it cannot be read.
    pub(crate) fn new(format: &str) -> Self {
        let items = StrftimeItems::new(format)
            .parse_to_owned()
            .unwrap_or_else(|_| {
                tracing::warn!("the clock's format {format:?} cannot be read; showing {FALLBACK}");
                StrftimeItems::new(FALLBACK)
                    .parse_to_owned()
                    .unwrap_or_default()
            });
        let every = if items.iter().any(shows_seconds) {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(60)
        };
        Self { items, every }
    }

    /// The time `now`, as the clock shows it.
    pub(crate) fn show<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        let mut shown = String::new();
        // Formatting fails only on an item the date lacks, which a date
        // with a time and a zone never does.
        if write!(shown, "{}", now.format_with_items(self.items.iter())).is_err() {
            shown.clear();
        }
        shown
    }

    /// How long from `now` until what the clock shows next changes: the
    /// start of the next minute, or of the next second.
    pub(crate) fn until_next<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Duration {
        // A leap second counts its nanoseconds past a billion.
        let into_second = Duration::from_nanos(u64::from(now.nanosecond().min(999_999_999)));
        let into = if self.every.as_secs() >= 60 {
            Duration::from_secs(u64::from(now.second())) + into_second
        } else {
            into_second
        };
        self.every.saturating_sub(into)
    }
}

/// Whether `item` writes something that changes within a minute.
fn shows_seconds(item: &Item<'_>) -> bool {
    match item {
        Item::Numeric(numeric, _) => matches!(
            numeric,
            Numeric::Second | Numeric::Nanosecond | Numeric::Timestamp
        ),
        Item::Fixed(fixed) => !matches!(
            fixed,
            Fixed::ShortMonthName
                | Fixed::LongMonthName
                | Fixed::ShortWeekdayName
                | Fixed::LongWeekdayName
                | Fixed::LowerAmPm
                | Fixed::UpperAmPm
                | Fixed::TimezoneName
                | Fixed::TimezoneOffsetColon
                | Fixed::TimezoneOffsetDoubleColon
                | Fixed::TimezoneOffsetTripleColon
                | Fixed::TimezoneOffsetColonZ
                | Fixed::TimezoneOffset
                | Fixed::TimezoneOffsetZ
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    /// 3 October 2026, 14:05:07.25, two hours east of Greenwich.
    fn now() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(2 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 3, 14, 5, 7)
            .unwrap()
            + chrono::Duration::milliseconds(250)
    }

    #[test]
    fn the_clock_uses_the_configured_format() {
        assert_eq!(Clock::new("%H:%M").show(&now()), "14:05");
        assert_eq!(
            Clock::new("%a %e %b, %l:%M %p").show(&now()),
            "Sat  3 Oct,  2:05 PM"
        );
        assert_eq!(
            Clock::new("%H:%M %Q").show(&now()),
            "14:05",
            "one it cannot read is hours and minutes"
        );
    }

    #[test]
    fn it_ticks_on_the_minute_or_the_second_as_its_format_shows() {
        assert_eq!(
            Clock::new("%H:%M").until_next(&now()),
            Duration::from_millis(52_750),
            "to 14:06:00"
        );
        for seconds in ["%H:%M:%S", "%T", "%c", "%s", "%+"] {
            assert_eq!(
                Clock::new(seconds).until_next(&now()),
                Duration::from_millis(750),
                "{seconds} shows seconds"
            );
        }
        assert_eq!(
            Clock::new("%A %p %Z").until_next(&now()),
            Duration::from_millis(52_750),
            "names change on the minute at most"
        );
    }
}
