//! The sound, as the sound server tells it: its outputs, and how loud each
//! is.
//!
//! Kept apart from the protocol it is read in, so that what the panel shows
//! of it is worked out, and tested, as plain values. The thread that talks
//! to the server fills these in (see `audio`), and the shell's loop only
//! ever sees them whole.

/// A volume of 100%, as the server counts it.
pub(crate) const NORM: u32 = 0x10000;

/// What the server holds, as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The output sound goes to unless a program says otherwise, by name.
    pub(crate) default_sink: Option<String>,
    pub(crate) sinks: Vec<Sink>,
}

/// An output the server plays to: a sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sink {
    /// The server's number for it, while it lasts.
    pub(crate) index: u32,
    /// Its name, which stays its own:
    /// `alsa_output.pci-0000_00_1f.3.analog-stereo`.
    pub(crate) name: String,
    /// Each channel's volume, as the server counts it: [`NORM`] is 100%.
    pub(crate) volume: Vec<u32>,
    pub(crate) muted: bool,
}

/// How loud a device is, and whether it is muted: what the panel shows of
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Level {
    /// Its loudest channel, in percent of [`NORM`].
    pub(crate) percent: u32,
    pub(crate) muted: bool,
}

impl Facts {
    /// The default output, if the server has one it can name.
    pub(crate) fn output(&self) -> Option<&Sink> {
        let name = self.default_sink.as_deref()?;
        self.sinks.iter().find(|sink| sink.name == name)
    }
}

impl Sink {
    pub(crate) fn level(&self) -> Level {
        Level {
            percent: percent(&self.volume),
            muted: self.muted,
        }
    }
}

/// How loud, in percent, a device whose channels are at `volume` is: its
/// loudest channel's, rounded, which is the number pavucontrol and `pactl`
/// show.
pub(crate) fn percent(volume: &[u32]) -> u32 {
    let loudest = u64::from(volume.iter().copied().max().unwrap_or(0));
    let norm = u64::from(NORM);
    u32::try_from((loudest * 100 + norm / 2) / norm).unwrap_or(u32::MAX)
}

/// The volume, in percent, that `notches` of the wheel (up positive) take
/// `now` to, in steps of `step`: to the next multiple of it that way, so
/// that 24 goes to 25 and then 30. Going up never passes `limit`, and never
/// pulls down a volume already past it, as one set louder elsewhere; going
/// down stops at silence.
pub(crate) fn stepped(now: u32, notches: i32, step: u32, limit: u32) -> u32 {
    let step = step.max(1);
    let by = notches.unsigned_abs().saturating_mul(step);
    if notches > 0 {
        if now >= limit {
            return now;
        }
        (now / step * step).saturating_add(by).min(limit)
    } else {
        now.div_ceil(step).saturating_mul(step).saturating_sub(by)
    }
}

/// `volume`, each channel scaled so that the loudest is at `percent`,
/// keeping the balance between them. A device silent on every channel is
/// put at `percent` on each.
pub(crate) fn scaled(volume: &[u32], percent: u32) -> Vec<u32> {
    let norm = u64::from(NORM);
    let target = (u64::from(percent) * norm + 50) / 100;
    let loudest = u64::from(volume.iter().copied().max().unwrap_or(0));
    volume
        .iter()
        .map(|&channel| {
            let scaled = match loudest {
                0 => target,
                loudest => (u64::from(channel) * target + loudest / 2) / loudest,
            };
            u32::try_from(scaled).unwrap_or(u32::MAX)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(index: u32, name: &str, volume: &[u32], muted: bool) -> Sink {
        Sink {
            index,
            name: name.to_owned(),
            volume: volume.to_vec(),
            muted,
        }
    }

    #[test]
    fn a_raw_volume_of_0x3d71_reads_as_24_percent() {
        assert_eq!(percent(&[0x3d71, 0x3d71]), 24);
        assert_eq!(percent(&[NORM]), 100);
        assert_eq!(percent(&[0]), 0);
        assert_eq!(percent(&[]), 0, "no channels is silent");
        assert_eq!(percent(&[NORM / 2, NORM]), 100, "the loudest channel");
        assert_eq!(percent(&[NORM * 3 / 2]), 150, "past 100% too");
        // pactl's own rounding, to the nearest: 327 is just under half a
        // percent, and 328 just over.
        assert_eq!(percent(&[327]), 0);
        assert_eq!(percent(&[328]), 1);
    }

    #[test]
    fn a_notch_snaps_to_the_next_step_and_stops_at_the_limit() {
        let up = |now, notches| stepped(now, notches, 5, 100);
        assert_eq!(up(24, 1), 25, "a notch snaps 24 to 25");
        assert_eq!(up(25, 1), 30);
        assert_eq!(up(24, 2), 30);
        assert_eq!(up(24, -1), 20);
        assert_eq!(up(25, -1), 20);
        assert_eq!(up(98, 1), 100, "a notch up from 98 stops at the limit");
        assert_eq!(up(98, 2), 100, "and two do too");
        assert_eq!(stepped(96, 1, 5, 98), 98, "a limit off the steps");
        assert_eq!(up(100, 1), 100);
        assert_eq!(up(120, 1), 120, "a notch up from 120 leaves it at 120");
        assert_eq!(up(120, -1), 115, "and a notch down takes it down");
        assert_eq!(up(3, -1), 0);
        assert_eq!(up(0, -3), 0, "silence is as low as it goes");
        assert_eq!(up(40, 0), 40);
        assert_eq!(stepped(24, 1, 1, 100), 25);
        assert_eq!(stepped(24, 1, 0, 100), 25, "a step of nothing is one");
    }

    #[test]
    fn scaling_keeps_the_balance_and_puts_the_loudest_where_asked() {
        assert_eq!(percent(&scaled(&[NORM, NORM / 2], 40)), 40);
        let balanced = scaled(&[NORM, NORM / 2], 50);
        assert_eq!(balanced, [NORM / 2, NORM / 4]);
        assert_eq!(scaled(&[0, 0], 30), [19661, 19661], "silent, put level");
        for percent_asked in [0, 1, 24, 99, 100, 150] {
            assert_eq!(
                percent(&scaled(&[0x3d71, 0x3d71], percent_asked)),
                percent_asked
            );
        }
        assert_eq!(scaled(&[], 50), Vec::<u32>::new());
    }

    #[test]
    fn the_output_is_the_default_sink_by_name() {
        let facts = Facts {
            default_sink: Some("two".to_owned()),
            sinks: vec![
                sink(1, "one", &[NORM], false),
                sink(2, "two", &[NORM / 4], true),
            ],
        };
        let output = facts.output().expect("the default");
        assert_eq!(output.index, 2);
        assert_eq!(
            output.level(),
            Level {
                percent: 25,
                muted: true
            }
        );
        let gone = Facts {
            default_sink: Some("three".to_owned()),
            ..facts.clone()
        };
        assert_eq!(gone.output(), None, "a default the server lists no sink of");
        assert_eq!(Facts::default().output(), None);
    }
}
