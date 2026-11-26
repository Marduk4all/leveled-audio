//! Rauschreduktion als Streaming-Stufe (STFT + Denoiser) mit Statistik je
//! Frame-Klasse aus Pass 1.

use leveled_audio_analysis::{FrameClass, power_scale};
use leveled_audio_dsp::{DenoiseParams, SpectralDenoiser, StftEngine};

/// Statistik der Rauschreduktion.
///
/// Pausen werden pro Frame betrachtet und über den **Median** ausgewertet:
/// Pausen-Frames im VAD-Hangover enthalten oft noch Sprachausklang oder Atmer,
/// die richtigerweise nicht abgesenkt werden und ein Energiemittel dominieren
/// würden. Für Sprache zählt dagegen der Energieverlust insgesamt.
#[derive(Debug, Clone, Default)]
pub struct DenoiseStats {
    /// Pegel je Pausen-Frame vorher (dB, Rohleistung) und Absenkung (dB).
    pause_level_db: Vec<f32>,
    pause_att_db: Vec<f32>,
    pub speech_in: f64,
    pub speech_out: f64,
}

fn median(v: &[f32]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    Some(s[s.len() / 2] as f64)
}

impl DenoiseStats {
    /// Typische Absenkung in Pausen (Median über die Frames, dB ≤ 0).
    pub fn attenuation_pauses_db(&self) -> Option<f64> {
        median(&self.pause_att_db)
    }
    /// Energieverlust der Sprache insgesamt (dB ≤ 0).
    pub fn attenuation_speech_db(&self) -> Option<f64> {
        (self.speech_in > 0.0 && self.speech_out > 0.0)
            .then(|| 10.0 * (self.speech_out / self.speech_in).log10())
    }
}

pub struct DenoiseStage {
    engine: StftEngine,
    denoiser: SpectralDenoiser,
    classes: Vec<FrameClass>,
    channels: usize,
    pub stats: DenoiseStats,
    scale: f64,
}

impl DenoiseStage {
    pub fn new(
        noise_power: &[Vec<f32>],
        fft_size: usize,
        sample_rate: u32,
        params: DenoiseParams,
        classes: Vec<FrameClass>,
    ) -> Self {
        let channels = noise_power.len();
        let engine = StftEngine::new(channels, fft_size, true);
        let hop_s = engine.hop() as f64 / sample_rate as f64;
        DenoiseStage {
            denoiser: SpectralDenoiser::new(noise_power, hop_s, params),
            engine,
            classes,
            channels,
            stats: DenoiseStats::default(),
            scale: power_scale(fft_size),
        }
    }

    pub fn latency(&self) -> usize {
        self.engine.latency()
    }

    /// Typischer Pegel in Pausen (Median, dBFS RMS) vor und nach der Rauschreduktion.
    pub fn pause_levels_db(&self) -> (Option<f64>, Option<f64>) {
        let offset = 10.0 * (self.scale / self.channels as f64).log10();
        let before = median(&self.stats.pause_level_db).map(|v| v + offset);
        let after = before
            .zip(self.stats.attenuation_pauses_db())
            .map(|(b, a)| b + a);
        (before, after)
    }

    pub fn process(&mut self, input: &[Vec<f32>], frames: usize, output: &mut [Vec<f32>]) {
        let DenoiseStage {
            engine,
            denoiser,
            classes,
            stats,
            ..
        } = self;
        engine.process(input, frames, output, |f| {
            let class = classes
                .get(f.index as usize)
                .copied()
                .unwrap_or(FrameClass::Other);
            let energy = |s: &[Vec<leveled_audio_dsp::Complex32>]| -> f64 {
                s.iter()
                    .map(|c| {
                        let last = c.len() - 1;
                        c.iter()
                            .enumerate()
                            .map(|(k, v)| {
                                let w = if k == 0 || k == last { 0.5 } else { 1.0 };
                                w * v.norm_sqr() as f64
                            })
                            .sum::<f64>()
                    })
                    .sum()
            };
            let before = energy(f.spectra);
            denoiser.process(f.spectra);
            let after = energy(f.spectra);
            match class {
                FrameClass::Pause if before > 0.0 && after > 0.0 => {
                    stats.pause_level_db.push((10.0 * before.log10()) as f32);
                    stats
                        .pause_att_db
                        .push((10.0 * (after / before).log10()) as f32);
                }
                FrameClass::Speech => {
                    stats.speech_in += before;
                    stats.speech_out += after;
                }
                _ => {}
            }
        });
    }
}
