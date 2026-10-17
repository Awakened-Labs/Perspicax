//! The test sound: two soft notes, rising, that the mixer's Test plays on
//! an output to show which it is.
//!
//! Each note fades in and out, so that the sound starts and ends silent,
//! and nothing clicks however the output takes it up and lets it go.

use std::f32::consts::TAU;

/// The samples a second the chime is made at.
pub(super) const RATE: u32 = 44_100;

/// The notes, in hertz, an E and the A above it; how long each sounds,
/// and fades in and out over, in seconds; and how loud each is at most.
const NOTES: [f32; 2] = [659.25, 880.0];
const NOTE: f32 = 0.18;
const FADE: f32 = 0.02;
const LOUD: f32 = 0.25;

/// The chime, as samples from -1 to 1, of one channel at [`RATE`].
pub(super) fn chime() -> Vec<f32> {
    let each = (NOTE * RATE as f32) as usize;
    let fade = (FADE * RATE as f32) as usize;
    NOTES
        .iter()
        .flat_map(|&pitch| {
            (0..each).map(move |at| {
                let into = at.min(each - 1 - at);
                let swell = (into as f32 / fade as f32).min(1.0);
                // Soft, the tail of each note falling away.
                let fall = 1.0 - 0.5 * at as f32 / each as f32;
                LOUD * swell * fall * (TAU * pitch * at as f32 / RATE as f32).sin()
            })
        })
        .collect()
}

/// `samples`, as a playback stream of floats takes them.
pub(super) fn bytes(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chime_starts_and_ends_silent_and_is_never_loud() {
        let chime = chime();
        let seconds = chime.len() as f32 / RATE as f32;
        assert!((seconds - 2.0 * NOTE).abs() < 0.001, "{seconds}");
        assert!(
            chime[0].abs() < 1e-6 && chime[chime.len() - 1].abs() < 1e-3,
            "silent at its ends"
        );
        let each = (NOTE * RATE as f32) as usize;
        assert!(
            chime[each - 1].abs() < 1e-3 && chime[each].abs() < 1e-6,
            "between its notes too"
        );
        let loudest = chime
            .iter()
            .fold(0.0_f32, |most, sample| most.max(sample.abs()));
        assert!(loudest <= LOUD && loudest > LOUD / 2.0, "{loudest}");
        assert_eq!(bytes(&chime[..2]).len(), 8, "four bytes a sample");
        assert_eq!(bytes(&[0.5])[..], 0.5_f32.to_le_bytes());
    }
}
