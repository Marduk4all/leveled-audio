//! Breitband-Kompressor für starke Dynamik (PLAN 3.12).
//!
//! Feed-forward im dB-Bereich mit Soft-Knee:
//! 1. Detektor: Kanalmittel der Leistung, Ein-Pol-Glättung (≈ RMS über `rms_ms`).
//! 2. Kennlinie: Absenkung `(1/ratio − 1) · (L − T)` oberhalb der Schwelle,
//!    quadratischer Übergang über `knee_db`.
//! 3. Attack/Release als Ein-Pol im dB-Bereich.
//! 4. Vorausschau: Das Audiosignal wird um `lookahead_ms` verzögert, der
//!    Detektor sieht das unverzögerte Signal. So steht die Absenkung schon,
//!    wenn ein lautes Ereignis beginnt.
//!
//! Stereo gekoppelt (ein Gain für alle Kanäle).

use crate::db_to_lin;

#[derive(Debug, Clone, Copy)]
pub struct CompressorParams {
    /// Schwelle in dBFS (RMS).
    pub threshold_dbfs: f64,
    /// Verhältnis ≥ 1.
    pub ratio: f64,
    pub knee_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub rms_ms: f64,
    pub lookahead_ms: f64,
}

impl Default for CompressorParams {
    fn default() -> Self {
        CompressorParams {
            threshold_dbfs: -20.0,
            ratio: 1.5,
            knee_db: 6.0,
            attack_ms: 5.0,
            release_ms: 150.0,
            rms_ms: 10.0,
            lookahead_ms: 5.0,
        }
    }
}

/// Ab dieser Absenkung zählt ein Sample als „komprimiert“ (Statistik).
const ACTIVE_DB: f64 = 0.5;

#[derive(Debug, Clone, Copy, Default)]
pub struct CompressorStats {
    /// Größte Absenkung in dB (≥ 0).
    pub max_reduction_db: f64,
    pub active_frames: u64,
    pub total_frames: u64,
}

impl CompressorStats {
    pub fn active_fraction(&self) -> f64 {
        if self.total_frames == 0 {
            0.0
        } else {
            self.active_frames as f64 / self.total_frames as f64
        }
    }
}

pub struct Compressor {
    p: CompressorParams,
    channels: usize,
    delay: Vec<Vec<f32>>,
    dpos: usize,
    look: usize,
    det_coeff: f64,
    att_coeff: f64,
    rel_coeff: f64,
    power: f64,
    gr_db: f64,
    stats: CompressorStats,
}

fn coeff(ms: f64, rate: u32) -> f64 {
    (-1.0 / (ms.max(0.01) / 1000.0 * rate as f64)).exp()
}

impl Compressor {
    pub fn new(channels: usize, sample_rate: u32, p: CompressorParams) -> Self {
        let look = (p.lookahead_ms / 1000.0 * sample_rate as f64).round() as usize;
        Compressor {
            p,
            channels,
            delay: vec![vec![0.0; look.max(1)]; channels],
            dpos: 0,
            look,
            det_coeff: coeff(p.rms_ms, sample_rate),
            att_coeff: coeff(p.attack_ms, sample_rate),
            rel_coeff: coeff(p.release_ms, sample_rate),
            power: 0.0,
            gr_db: 0.0,
            stats: CompressorStats::default(),
        }
    }

    /// Verzögerung des Audiosignals in Samples.
    pub fn latency(&self) -> usize {
        self.look
    }

    pub fn stats(&self) -> CompressorStats {
        self.stats
    }

    /// Statische Kennlinie: Gain-Änderung in dB (≤ 0) für den Pegel `level_db`.
    pub fn static_gain_db(&self, level_db: f64) -> f64 {
        let p = &self.p;
        let slope = 1.0 / p.ratio.max(1.0) - 1.0;
        let over = level_db - p.threshold_dbfs;
        let half = p.knee_db / 2.0;
        if over <= -half {
            0.0
        } else if over >= half || p.knee_db <= 0.0 {
            slope * over
        } else {
            slope * (over + half).powi(2) / (2.0 * p.knee_db)
        }
    }

    /// Verarbeitet `frames` Frames; `output` erhält ebenso viele, um
    /// [`latency`](Self::latency) verzögert.
    pub fn process(&mut self, input: &[Vec<f32>], output: &mut [Vec<f32>], frames: usize) {
        let ch = self.channels;
        for o in output.iter_mut() {
            o.resize(frames, 0.0);
        }
        for n in 0..frames {
            let mut pw = 0f64;
            for c in 0..ch {
                let x = input[c][n] as f64;
                pw += x * x;
            }
            pw /= ch as f64;
            self.power = self.det_coeff * self.power + (1.0 - self.det_coeff) * pw;
            let level = if self.power > 1e-20 {
                10.0 * self.power.log10()
            } else {
                -200.0
            };
            let target = self.static_gain_db(level);
            let k = if target < self.gr_db {
                self.att_coeff
            } else {
                self.rel_coeff
            };
            self.gr_db = k * self.gr_db + (1.0 - k) * target;
            let g = db_to_lin(self.gr_db) as f32;
            for c in 0..ch {
                let x = input[c][n];
                let delayed = if self.look == 0 {
                    x
                } else {
                    let d = self.delay[c][self.dpos];
                    self.delay[c][self.dpos] = x;
                    d
                };
                output[c][n] = delayed * g;
            }
            if self.look > 0 {
                self.dpos = (self.dpos + 1) % self.look;
            }
            self.stats.total_frames += 1;
            if -self.gr_db >= ACTIVE_DB {
                self.stats.active_frames += 1;
            }
            self.stats.max_reduction_db = self.stats.max_reduction_db.max(-self.gr_db);
        }
    }

    /// Verhältnis aus der Stärke 0–1 (PLAN 3.12): 0 → 1:1, 0,4 → 1,5:1, 0,6 → 2:1, 1 → 6:1.
    pub fn ratio_for_amount(amount: f64) -> f64 {
        1.0 / (1.0 - amount.clamp(0.0, 1.0) * 5.0 / 6.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f64, level_dbfs: f64) -> Vec<f32> {
        let a = db_to_lin(level_dbfs) * 2f64.sqrt();
        (0..(rate as f64 * secs) as usize)
            .map(|i| {
                (a * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin()) as f32
            })
            .collect()
    }

    fn rms_db(x: &[f32]) -> f64 {
        10.0 * (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).log10()
    }

    #[test]
    fn ratio_mapping_and_curve() {
        assert!((Compressor::ratio_for_amount(0.0) - 1.0).abs() < 1e-12);
        assert!((Compressor::ratio_for_amount(0.6) - 2.0).abs() < 1e-9);
        assert!((Compressor::ratio_for_amount(1.0) - 6.0).abs() < 1e-9);
        assert!((Compressor::ratio_for_amount(0.4) - 1.5).abs() < 1e-9);
        let c = Compressor::new(
            1,
            48_000,
            CompressorParams {
                threshold_dbfs: -20.0,
                ratio: 4.0,
                ..Default::default()
            },
        );
        assert_eq!(c.static_gain_db(-30.0), 0.0);
        assert!((c.static_gain_db(-8.0) + 9.0).abs() < 1e-9);
        // Knie stetig.
        assert!((c.static_gain_db(-17.0) - c.static_gain_db(-16.999)).abs() < 1e-3);
    }

    #[test]
    fn steady_state_level_and_latency() {
        let rate = 48_000;
        let p = CompressorParams {
            threshold_dbfs: -20.0,
            ratio: 2.0,
            ..Default::default()
        };
        let mut c = Compressor::new(2, rate, p);
        let x = tone(rate, 1.0, -8.0);
        let input = vec![x.clone(), x.clone()];
        let mut out = vec![Vec::new(), Vec::new()];
        c.process(&input, &mut out, x.len());
        // 12 dB über der Schwelle, 2:1 ⇒ 6 dB Absenkung.
        let tail = &out[0][x.len() / 2..];
        assert!((rms_db(tail) - (-14.0)).abs() < 0.3, "{}", rms_db(tail));
        assert_eq!(c.latency(), 240);
        assert!(out[0][..240].iter().all(|v| *v == 0.0));
        assert!((c.stats().max_reduction_db - 6.0).abs() < 0.3);
    }

    #[test]
    fn below_threshold_is_untouched() {
        let rate = 44_100;
        let mut c = Compressor::new(
            1,
            rate,
            CompressorParams {
                threshold_dbfs: -10.0,
                ratio: 6.0,
                ..Default::default()
            },
        );
        let x = tone(rate, 0.5, -30.0);
        let mut out = vec![Vec::new()];
        c.process(std::slice::from_ref(&x), &mut out, x.len());
        let l = c.latency();
        for i in l..x.len() {
            assert_eq!(out[0][i], x[i - l]);
        }
        assert_eq!(c.stats().active_frames, 0);
    }
}
