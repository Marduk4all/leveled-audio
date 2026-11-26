//! Rauschschätzung nach dem Minimum-Statistics-Prinzip (nach Martin, 2001),
//! vereinfacht: geglättete Leistung je Bin, Minimum über ein Fenster aus
//! `U` Teilfenstern, konstante Bias-Korrektur.
//!
//! Dient als Fallback, wenn keine verwertbaren Pausen gefunden werden. Es
//! werden regelmäßig Momentaufnahmen gespeichert; das statische Profil ist
//! der Median über die Momentaufnahmen.

use leveled_audio_dsp::Complex32;

const SUBWINDOWS: usize = 8;
const ALPHA: f32 = 0.85;
/// Kompensiert die Unterschätzung durch die Minimumbildung
/// (für ALPHA = 0,85 und ≈ 2,5 s Fenster kalibriert, siehe Test).
const BIAS: f32 = 1.75;

pub struct MinStatTracker {
    channels: usize,
    bins: usize,
    sub_len: usize,
    sub_pos: usize,
    smoothed: Vec<Vec<f32>>,
    sub_min: Vec<Vec<f32>>,
    ring: Vec<Vec<Vec<f32>>>,
    ring_pos: usize,
    frames: u64,
    warmup: u64,
    snapshot_every: u64,
    snapshots: Vec<Vec<Vec<f32>>>,
}

impl MinStatTracker {
    /// `window_s`: Minimum-Fenster (≈ 1,5 s). `snapshot_s`: Abstand der
    /// Momentaufnahmen. `hop_s`: Frame-Abstand.
    pub fn new(channels: usize, bins: usize, hop_s: f64, window_s: f64, snapshot_s: f64) -> Self {
        let sub_len = ((window_s / hop_s / SUBWINDOWS as f64).round() as usize).max(1);
        MinStatTracker {
            channels,
            bins,
            sub_len,
            sub_pos: 0,
            smoothed: vec![vec![0.0; bins]; channels],
            sub_min: vec![vec![f32::INFINITY; bins]; channels],
            ring: vec![vec![vec![f32::INFINITY; bins]; channels]; SUBWINDOWS],
            ring_pos: 0,
            frames: 0,
            warmup: (sub_len * SUBWINDOWS) as u64,
            snapshot_every: ((snapshot_s / hop_s).round() as u64).max(1),
            snapshots: Vec::new(),
        }
    }

    pub fn push(&mut self, spectra: &[Vec<Complex32>]) {
        for c in 0..self.channels {
            let sm = &mut self.smoothed[c];
            let mn = &mut self.sub_min[c];
            for k in 0..self.bins {
                let p = spectra[c][k].norm_sqr();
                sm[k] = if self.frames == 0 {
                    p
                } else {
                    ALPHA * sm[k] + (1.0 - ALPHA) * p
                };
                mn[k] = mn[k].min(sm[k]);
            }
        }
        self.frames += 1;
        self.sub_pos += 1;
        if self.sub_pos == self.sub_len {
            self.sub_pos = 0;
            for c in 0..self.channels {
                self.ring[self.ring_pos][c].copy_from_slice(&self.sub_min[c]);
                self.sub_min[c].iter_mut().for_each(|v| *v = f32::INFINITY);
            }
            self.ring_pos = (self.ring_pos + 1) % SUBWINDOWS;
        }
        if self.frames >= self.warmup && self.frames.is_multiple_of(self.snapshot_every) {
            let s = self.estimate();
            self.snapshots.push(s);
        }
    }

    /// Aktuelle Schätzung je Kanal und Bin (Leistung).
    pub fn estimate(&self) -> Vec<Vec<f32>> {
        (0..self.channels)
            .map(|c| {
                (0..self.bins)
                    .map(|k| {
                        let mut m = self.sub_min[c][k];
                        for r in &self.ring {
                            m = m.min(r[c][k]);
                        }
                        if m.is_finite() { m * BIAS } else { 0.0 }
                    })
                    .collect()
            })
            .collect()
    }

    /// Statisches Profil: Median über die Momentaufnahmen (bzw. aktuelle
    /// Schätzung, wenn die Datei kürzer als das Fenster ist).
    pub fn profile(&self) -> Vec<Vec<f32>> {
        if self.snapshots.is_empty() {
            return self.estimate();
        }
        let n = self.snapshots.len();
        let mut col = vec![0f32; n];
        (0..self.channels)
            .map(|c| {
                (0..self.bins)
                    .map(|k| {
                        for (i, s) in self.snapshots.iter().enumerate() {
                            col[i] = s[c][k];
                        }
                        col.sort_by(f32::total_cmp);
                        col[n / 2]
                    })
                    .collect()
            })
            .collect()
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exponentialverteilte Leistung (wie |X|² von Gaußrauschen), Mittelwert 1.
    fn exp_frames(n: usize, bins: usize, seed: u64) -> Vec<Vec<Complex32>> {
        let mut s = seed;
        let mut u = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        (0..n)
            .map(|_| {
                (0..bins)
                    .map(|_| Complex32::new((-u().ln()).sqrt() as f32, 0.0))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn bias_calibrated_for_stationary_noise() {
        let hop = 512.0 / 48_000.0;
        let mut t = MinStatTracker::new(1, 256, hop, 2.5, 0.5);
        for f in exp_frames(3000, 256, 99) {
            t.push(&[f]);
        }
        let p = t.profile();
        let mean = p[0].iter().map(|v| *v as f64).sum::<f64>() / 256.0;
        let db = 10.0 * mean.log10();
        assert!(db.abs() < 1.0, "Bias {db:.2} dB");
    }

    #[test]
    fn ignores_intermittent_loud_bursts() {
        let hop = 512.0 / 48_000.0;
        let mut t = MinStatTracker::new(1, 64, hop, 2.5, 0.5);
        for (i, mut f) in exp_frames(3000, 64, 7).into_iter().enumerate() {
            // 60 % der Zeit "Sprache" mit +20 dB.
            if (i / 40) % 5 < 3 {
                f.iter_mut().for_each(|v| *v *= 10.0);
            }
            t.push(&[f]);
        }
        let mean = t.profile()[0].iter().map(|v| *v as f64).sum::<f64>() / 64.0;
        let db = 10.0 * mean.log10();
        assert!(db.abs() < 3.0, "{db:.2} dB");
    }
}
