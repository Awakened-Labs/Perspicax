//! The spectrum of what plays, for the mixer's moving bars: as winamp drew
//! it, the loudness of each of [`BANDS`] bands of pitch, from the deep end
//! to the shrill one, spaced as the ear spaces them.
//!
//! The thread records the default output's monitor, one channel at
//! [`RATE`], and each [`HOP`] samples takes the last [`SIZE`], shaped by a
//! Hann window, through a fast Fourier transform. Each band is the loudest
//! bin between its edges, log-spaced from [`LOWEST`] to [`HIGHEST`] hertz,
//! read in decibels below full scale and put between 0, at [`FLOOR`] and
//! below, and 1 at full scale. A transform of its own, and a small one: a
//! crate's would bring more than the bars need.

use std::f32::consts::PI;

/// How many bars there are.
pub(crate) const BANDS: usize = 16;
/// The rate the monitor is recorded at, in samples a second: a band reaches
/// up to half of it.
pub(super) const RATE: u32 = 22_050;
/// How many samples each transform takes, and how many come between one
/// and the next: about 43 a second.
const SIZE: usize = 1024;
pub(super) const HOP: usize = 512;
/// The deepest and the shrillest a band reaches, in hertz.
const LOWEST: f32 = 60.0;
const HIGHEST: f32 = 10_000.0;
/// The quietest a bar shows, in decibels below full scale.
const FLOOR: f32 = 60.0;

/// The samples recorded, and how each transform reads them.
pub(super) struct Spectrum {
    /// The last [`SIZE`] samples, the oldest first.
    samples: Vec<f32>,
    /// How many have come since the last transform.
    since: usize,
    /// The window each transform's samples are shaped by.
    window: Vec<f32>,
    /// Each band's first bin and the one past its last.
    edges: [(usize, usize); BANDS],
}

impl Spectrum {
    pub(super) fn new() -> Self {
        let window = (0..SIZE)
            .map(|at| 0.5 - 0.5 * (2.0 * PI * at as f32 / SIZE as f32).cos())
            .collect();
        let bin = |hertz: f32| hertz * SIZE as f32 / RATE as f32;
        let edges = std::array::from_fn(|band| {
            let at = |band: usize| LOWEST * (HIGHEST / LOWEST).powf(band as f32 / BANDS as f32);
            let first = bin(at(band)).round() as usize;
            let past = (bin(at(band + 1)).round() as usize).max(first + 1);
            (first, past.min(SIZE / 2))
        });
        Self {
            samples: vec![0.0; SIZE],
            since: 0,
            window,
            edges,
        }
    }

    /// Take in `samples`, as they came. The bands of the newest transform,
    /// if a hop's worth came for one.
    pub(super) fn feed(&mut self, samples: &[f32]) -> Option<[f32; BANDS]> {
        let mut bands = None;
        for &sample in samples {
            self.samples.rotate_left(1);
            self.samples[SIZE - 1] = sample;
            self.since += 1;
            if self.since == HOP {
                self.since = 0;
                bands = Some(self.bands());
            }
        }
        bands
    }

    /// Each band's loudness, 0 to 1, in the last [`SIZE`] samples.
    fn bands(&self) -> [f32; BANDS] {
        let mut real: Vec<f32> = self
            .samples
            .iter()
            .zip(&self.window)
            .map(|(sample, shape)| sample * shape)
            .collect();
        let mut imaginary = vec![0.0; SIZE];
        transform(&mut real, &mut imaginary);
        // A full-scale sine, through the window, peaks at a quarter of the
        // samples.
        let full = SIZE as f32 / 4.0;
        self.edges.map(|(first, past)| {
            let loudest = (first..past)
                .map(|bin| real[bin].hypot(imaginary[bin]))
                .fold(0.0, f32::max);
            let decibels = 20.0 * (loudest / full).max(f32::MIN_POSITIVE).log10();
            ((decibels + FLOOR) / FLOOR).clamp(0.0, 1.0)
        })
    }
}

/// The discrete Fourier transform of `real` and `imaginary`, in place: an
/// iterative radix-2 transform, so their length is a power of two.
fn transform(real: &mut [f32], imaginary: &mut [f32]) {
    let size = real.len();
    debug_assert!(size.is_power_of_two() && imaginary.len() == size);
    let bits = size.trailing_zeros();
    for at in 0..size {
        let swapped = at.reverse_bits() >> (usize::BITS - bits);
        if swapped > at {
            real.swap(at, swapped);
            imaginary.swap(at, swapped);
        }
    }
    let mut span = 2;
    while span <= size {
        let angle = -2.0 * PI / span as f32;
        for start in (0..size).step_by(span) {
            for k in 0..span / 2 {
                let (sin, cos) = (angle * k as f32).sin_cos();
                let (even, odd) = (start + k, start + k + span / 2);
                let turned_re = real[odd] * cos - imaginary[odd] * sin;
                let turned_im = real[odd] * sin + imaginary[odd] * cos;
                real[odd] = real[even] - turned_re;
                imaginary[odd] = imaginary[even] - turned_im;
                real[even] += turned_re;
                imaginary[even] += turned_im;
            }
        }
        span *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine of `hertz` at `amplitude`, `count` samples long.
    fn sine(hertz: f32, amplitude: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|at| amplitude * (2.0 * PI * hertz * at as f32 / RATE as f32).sin())
            .collect()
    }

    /// The band holding `hertz`.
    fn band_of(hertz: f32) -> usize {
        let spectrum = Spectrum::new();
        let bin = (hertz * SIZE as f32 / RATE as f32).round() as usize;
        spectrum
            .edges
            .iter()
            .position(|&(first, past)| (first..past).contains(&bin))
            .expect("a band")
    }

    #[test]
    fn a_1_khz_sine_lights_the_band_holding_1_khz() {
        let mut spectrum = Spectrum::new();
        let bands = spectrum
            .feed(&sine(1000.0, 0.5, SIZE))
            .expect("a hop's worth");
        let lit = band_of(1000.0);
        let loudest = (0..BANDS)
            .max_by(|&a, &b| bands[a].total_cmp(&bands[b]))
            .expect("bands");
        assert_eq!(loudest, lit, "{bands:?}");
        assert!(
            bands[lit] > 0.8,
            "half of full scale is -6 dB: {}",
            bands[lit]
        );
        assert!(bands[0] < 0.3, "the deepest stays low: {bands:?}");
    }

    #[test]
    fn silence_lights_nothing() {
        let mut spectrum = Spectrum::new();
        let bands = spectrum.feed(&vec![0.0; SIZE]).expect("a hop's worth");
        assert_eq!(bands, [0.0; BANDS]);
    }

    #[test]
    fn a_transform_comes_each_hop_and_not_between() {
        let mut spectrum = Spectrum::new();
        assert!(spectrum.feed(&vec![0.1; HOP - 1]).is_none());
        assert!(spectrum.feed(&[0.1]).is_some());
        assert!(spectrum.feed(&vec![0.1; HOP / 2]).is_none());
    }

    #[test]
    fn the_bands_rise_from_deep_to_shrill_each_at_least_a_bin_wide() {
        let spectrum = Spectrum::new();
        for pair in spectrum.edges.windows(2) {
            assert!(pair[0].0 < pair[0].1, "{pair:?}");
            assert!(pair[0].0 <= pair[1].0, "{pair:?}");
        }
        assert!(band_of(100.0) < band_of(1000.0));
        assert!(band_of(1000.0) < band_of(8000.0));
    }

    #[test]
    fn the_transform_finds_the_bin_of_a_whole_number_of_cycles() {
        let mut real: Vec<f32> = (0..64)
            .map(|at| (2.0 * PI * 5.0 * at as f32 / 64.0).cos())
            .collect();
        let mut imaginary = vec![0.0; 64];
        transform(&mut real, &mut imaginary);
        let magnitude = |bin: usize| real[bin].hypot(imaginary[bin]);
        assert!((magnitude(5) - 32.0).abs() < 1e-3, "{}", magnitude(5));
        assert!(magnitude(4) < 1e-3 && magnitude(6) < 1e-3);
    }
}
