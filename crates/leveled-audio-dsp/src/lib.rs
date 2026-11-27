//! DSP-Bausteine für Leveled Audio. Keine Datei-I/O, keine Threads.
//!
//! Konventionen: Audio liegt planar als `Vec<f32>` pro Kanal vor.
//! Zustände, die über viele Samples akkumulieren, werden in `f64` geführt.
//! Alle Bausteine sind deterministisch (keine Zufallsquellen außer seeded PRNG,
//! feste Summationsreihenfolge).

pub mod compressor;
pub mod denoise;
pub mod dither;
pub mod highpass;
pub mod limiter;
pub mod oversampler;
pub mod stft;

pub use compressor::{Compressor, CompressorParams, CompressorStats};
pub use denoise::{DenoiseParams, GainRule, SpectralDenoiser};
pub use dither::TpdfDither;
pub use highpass::Highpass;
pub use limiter::{LimiterParams, LimiterStats, TruePeakLimiter};
pub use oversampler::PeakOversampler;
pub use stft::{Complex32, DelayLine, StftEngine, StftFrame, auto_fft_size};

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
