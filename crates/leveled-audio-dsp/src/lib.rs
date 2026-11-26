//! DSP-Bausteine für leveled-audio. Keine Datei-I/O, keine Threads.
//!
//! Konventionen: Audio liegt planar als `Vec<f32>` pro Kanal vor.
//! Zustände, die über viele Samples akkumulieren, werden in `f64` geführt.
//! Alle Bausteine sind deterministisch (keine Zufallsquellen außer seeded PRNG,
//! feste Summationsreihenfolge).

pub mod dither;
pub mod limiter;
pub mod oversampler;

pub use dither::TpdfDither;
pub use limiter::{LimiterParams, LimiterStats, TruePeakLimiter};
pub use oversampler::PeakOversampler;

/// Dezibel → linearer Faktor.
#[inline]
pub fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Linearer Faktor → Dezibel (−∞ für 0).
#[inline]
pub fn lin_to_db(lin: f64) -> f64 {
    20.0 * lin.log10()
}

/// Wendet einen konstanten Gain auf alle Kanäle an.
pub fn apply_gain(buf: &mut [Vec<f32>], frames: usize, gain: f32) {
    if gain == 1.0 {
        return;
    }
    for ch in buf.iter_mut() {
        for x in &mut ch[..frames] {
            *x *= gain;
        }
    }
}
