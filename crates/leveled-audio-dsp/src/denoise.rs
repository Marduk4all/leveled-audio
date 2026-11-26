//! Spektrale Rauschreduktion mit festem Rauschprofil.
//!
//! Gain-Regeln:
//! - [`GainRule::WienerDd`] (Default): Wiener-Gain mit Decision-Directed-
//!   A-priori-SNR (Ephraim/Malah). Das A-priori-SNR wird über die Zeit
//!   geglättet, was Musical Noise stark reduziert.
//! - [`GainRule::Subtract`]: Power Subtraction `sqrt(max(1 − α·λ/|X|², β))`,
//!   näher am klassischen „Subtract“-Verhalten.
//!
//! Maßnahmen gegen Musical Noise: Decision-Directed-Schätzung, Gain-Untergrenze
//! (max. Absenkung) und asymmetrische Zeitglättung (sofortiger Anstieg,
//! Absinken mit 40 ms). Eine Glättung über die Frequenz ist verfügbar, aber per
//! Default aus: Sie senkt bei 23-Hz-Bins die Obertöne der Stimme mit ab und
//! erhöht die Sprachverzerrung deutlich (gemessen), bei kaum weniger Musical Noise.

use crate::stft::Complex32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainRule {
    WienerDd,
    Subtract,
}

#[derive(Debug, Clone, Copy)]
pub struct DenoiseParams {
    /// Maximale Absenkung in dB (Gain-Untergrenze).
    pub reduction_db: f64,
    /// Faktor auf das Rauschprofil.
    pub oversubtraction: f64,
    pub gain_rule: GainRule,
    /// Spectral Floor β (nur `Subtract`), relativ zum Rauschprofil (Leistung).
    pub floor: f64,
    /// Glättungsfaktor des Decision-Directed-Schätzers.
    pub dd_alpha: f64,
    /// Zeitkonstante, mit der der Gain absinkt (ms). Anstieg ist sofort.
    pub release_ms: f64,
    /// Breite der Frequenzglättung in Oktaven (0 = aus).
    pub freq_smooth_oct: f64,
    /// Gemeinsamer Gain für alle Kanäle (aus Summenleistung).
    pub stereo_link: bool,
}

impl Default for DenoiseParams {
    fn default() -> Self {
        DenoiseParams {
            reduction_db: 12.0,
            oversubtraction: 1.3,
            gain_rule: GainRule::WienerDd,
            floor: 0.05,
            dd_alpha: 0.98,
            release_ms: 40.0,
            freq_smooth_oct: 0.0,
            stereo_link: false,
        }
    }
}

pub struct SpectralDenoiser {
    p: DenoiseParams,
    bins: usize,
    /// Rauschleistung je Kanal und Bin, bereits mit Oversubtraction skaliert.
    noise: Vec<Vec<f32>>,
    /// G²·|X|² des Vorframes (für Decision-Directed).
    prev_clean: Vec<Vec<f32>>,
    prev_gain: Vec<Vec<f32>>,
    gmin: f32,
    beta: f32,
    rel: f32,
    /// Halbe Fensterbreite der Box-Glättung je Bin.
    half_width: Vec<usize>,
    // Arbeitsspeicher
    pow: Vec<f32>,
    gain: Vec<f32>,
    logg: Vec<f32>,
    prefix: Vec<f64>,
    first: bool,
}

impl SpectralDenoiser {
    /// `noise_power`: Rauschprofil je Kanal (Leistung je Bin, `fft_size/2+1` Bins).
    /// `hop_seconds`: Frame-Abstand, für die Zeitglättung.
    pub fn new(noise_power: &[Vec<f32>], hop_seconds: f64, p: DenoiseParams) -> Self {
        let ch = noise_power.len();
        let bins = noise_power[0].len();
        let a = p.oversubtraction as f32;
        let noise: Vec<Vec<f32>> = noise_power
            .iter()
            .map(|c| c.iter().map(|v| (v * a).max(1e-30)).collect())
            .collect();
        // Box-Glättung zweimal mit halber Breite ⇒ Dreiecksfenster der Gesamtbreite.
        let ratio = 2f64.powf(p.freq_smooth_oct / 2.0) - 1.0;
        let half_width = (0..bins)
            .map(|k| {
                if p.freq_smooth_oct <= 0.0 {
                    0
                } else {
                    ((k as f64 * ratio / 2.0).round() as usize).max(1)
                }
            })
            .collect();
        SpectralDenoiser {
            bins,
            noise,
            prev_clean: vec![vec![0.0; bins]; ch],
            prev_gain: vec![vec![1.0; bins]; ch],
            gmin: 10f64.powf(-p.reduction_db / 20.0) as f32,
            beta: p.floor.max(0.0).sqrt() as f32,
            rel: (-hop_seconds / (p.release_ms.max(0.1) / 1000.0)).exp() as f32,
            half_width,
            pow: vec![0.0; bins],
            gain: vec![0.0; bins],
            logg: vec![0.0; bins],
            prefix: vec![0.0; bins + 1],
            first: true,
            p,
        }
    }

    pub fn bins(&self) -> usize {
        self.bins
    }

    /// Verarbeitet einen Frame (alle Kanäle) in place.
    pub fn process(&mut self, spectra: &mut [Vec<Complex32>]) {
        let ch = spectra.len();
        if self.p.stereo_link && ch > 1 {
            for k in 0..self.bins {
                self.pow[k] = spectra.iter().map(|s| s[k].norm_sqr()).sum();
            }
            let lambda: Vec<f32> = (0..self.bins)
                .map(|k| self.noise.iter().map(|n| n[k]).sum())
                .collect();
            self.compute_gain(0, &lambda);
            for s in spectra.iter_mut() {
                for (v, g) in s.iter_mut().zip(&self.gain) {
                    *v *= *g;
                }
            }
        } else {
            for (c, s) in spectra.iter_mut().enumerate() {
                for k in 0..self.bins {
                    self.pow[k] = s[k].norm_sqr();
                }
                let lambda = std::mem::take(&mut self.noise[c]);
                self.compute_gain(c, &lambda);
                self.noise[c] = lambda;
                for (v, g) in s.iter_mut().zip(&self.gain) {
                    *v *= *g;
                }
            }
        }
        self.first = false;
    }

    /// Berechnet `self.gain` aus `self.pow` und dem Rauschprofil `lambda`.
    /// `state` wählt den Zustandsspeicher (Kanal bzw. 0 bei Stereo-Link).
    fn compute_gain(&mut self, state: usize, lambda: &[f32]) {
        let gmin = self.gmin;
        let a = self.p.dd_alpha as f32;
        // 1. Roher Gain
        for k in 0..self.bins {
            let pw = self.pow[k];
            let lam = lambda[k];
            let g = match self.p.gain_rule {
                GainRule::WienerDd => {
                    let gamma = pw / lam;
                    let ml = (gamma - 1.0).max(0.0);
                    let xi = if self.first {
                        ml
                    } else {
                        a * self.prev_clean[state][k] / lam + (1.0 - a) * ml
                    };
                    xi / (1.0 + xi)
                }
                GainRule::Subtract => {
                    let r = if pw > 0.0 { 1.0 - lam / pw } else { 0.0 };
                    r.max(self.beta * self.beta).sqrt()
                }
            };
            self.gain[k] = g.max(gmin);
        }
        // 2. Frequenzglättung im log-Bereich (zweimal Box ⇒ Dreieck).
        if self.p.freq_smooth_oct > 0.0 {
            for k in 0..self.bins {
                self.logg[k] = self.gain[k].ln();
            }
            for _ in 0..2 {
                self.prefix[0] = 0.0;
                for k in 0..self.bins {
                    self.prefix[k + 1] = self.prefix[k] + self.logg[k] as f64;
                }
                for k in 0..self.bins {
                    let w = self.half_width[k];
                    let lo = k.saturating_sub(w);
                    let hi = (k + w + 1).min(self.bins);
                    self.logg[k] = ((self.prefix[hi] - self.prefix[lo]) / (hi - lo) as f64) as f32;
                }
            }
            for k in 0..self.bins {
                self.gain[k] = self.logg[k].exp().max(gmin);
            }
        }
        // 3. Zeitglättung: sofortiger Anstieg, langsames Absinken.
        let rel = self.rel;
        let prev = &mut self.prev_gain[state];
        for k in 0..self.bins {
            let g = self.gain[k];
            let s = if g >= prev[k] {
                g
            } else {
                g + (prev[k] - g) * rel
            };
            let s = s.clamp(gmin, 1.0);
            self.gain[k] = s;
            prev[k] = s;
        }
        // 4. Zustand für Decision-Directed
        let pc = &mut self.prev_clean[state];
        for k in 0..self.bins {
            pc[k] = self.gain[k] * self.gain[k] * self.pow[k];
        }
    }

    /// Gain des letzten Frames (für Tests und Schattenfilterung).
    pub fn last_gain(&self) -> &[f32] {
        &self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pows: &[f32]) -> Vec<Complex32> {
        pows.iter().map(|p| Complex32::new(p.sqrt(), 0.0)).collect()
    }

    #[test]
    fn noise_only_is_attenuated_to_floor() {
        let bins = 64;
        let noise = vec![vec![1.0f32; bins]];
        let mut d = SpectralDenoiser::new(&noise, 0.01, DenoiseParams::default());
        let mut last = 0.0;
        for _ in 0..200 {
            let mut s = vec![frame(&vec![1.0; bins])];
            d.process(&mut s);
            last = s[0][10].norm_sqr();
        }
        let db = 10.0 * (last as f64).log10();
        assert!((db + 12.0).abs() < 0.1, "{db}");
    }

    #[test]
    fn strong_signal_passes_almost_unchanged() {
        let bins = 64;
        let noise = vec![vec![1.0f32; bins]];
        let mut d = SpectralDenoiser::new(&noise, 0.01, DenoiseParams::default());
        let mut g = 0.0;
        for _ in 0..50 {
            let mut s = vec![frame(&vec![10_000.0; bins])];
            d.process(&mut s);
            g = s[0][20].norm_sqr() / 10_000.0;
        }
        assert!(10.0 * (g as f64).log10() > -0.1, "{g}");
    }

    #[test]
    fn gain_never_below_floor_and_both_rules_work() {
        for rule in [GainRule::WienerDd, GainRule::Subtract] {
            let p = DenoiseParams {
                gain_rule: rule,
                reduction_db: 20.0,
                ..Default::default()
            };
            let bins = 128;
            let noise = vec![vec![1.0f32; bins]; 2];
            let mut d = SpectralDenoiser::new(&noise, 0.01, p);
            for f in 0..100 {
                let pw: Vec<f32> = (0..bins)
                    .map(|k| ((k * 7 + f * 13) % 17) as f32 * 0.3)
                    .collect();
                let mut s = vec![frame(&pw), frame(&pw)];
                d.process(&mut s);
                for &g in d.last_gain() {
                    assert!((0.0999..=1.0).contains(&g), "{rule:?}: {g}");
                }
            }
        }
    }

    #[test]
    fn stereo_link_applies_identical_gain() {
        let bins = 32;
        let noise = vec![vec![1.0f32; bins]; 2];
        let p = DenoiseParams {
            stereo_link: true,
            ..Default::default()
        };
        let mut d = SpectralDenoiser::new(&noise, 0.01, p);
        let mut s = vec![frame(&vec![4.0; bins]), frame(&vec![0.5; bins])];
        d.process(&mut s);
        let gl = s[0][5].re / 2.0;
        let gr = s[1][5].re / 0.5f32.sqrt();
        assert!((gl - gr).abs() < 1e-6);
    }
}
