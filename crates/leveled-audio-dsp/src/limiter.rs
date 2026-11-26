//! True-Peak-Limiter mit Lookahead.
//!
//! Gain-Pfad pro Sample:
//! 1. `r[t]` = benötigter Gain für das Intervall `(t, t+1)` (Oversampling-Detektion
//!    plus die beiden Stützsamples), `min(1, ceiling / peak)`.
//! 2. `m[t] = min(r[t ..= t+L])` (gleitendes Minimum über L+1 Werte).
//! 3. Release: sofortiges Absenken, Erholung per Ein-Pol-Filter.
//! 4. `G[t]` = Mittelwert der letzten L Werte aus 3.
//!
//! Da jeder der gemittelten Werte `r[t]` und `r[t+1]` enthält, gilt
//! `G[t] <= min(r[t], r[t+1])`: der Gain erreicht den Zielwert vor dem Peak.
//! Das Audiosignal wird um `D + L` Samples verzögert (D = Detektionsverzögerung).

use crate::oversampler::PeakOversampler;
use crate::{db_to_lin, lin_to_db};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub struct LimiterParams {
    pub ceiling_dbtp: f64,
    pub lookahead_ms: f64,
    pub release_ms: f64,
    pub oversampling: usize,
}

impl Default for LimiterParams {
    fn default() -> Self {
        LimiterParams {
            ceiling_dbtp: -1.0,
            lookahead_ms: 5.0,
            release_ms: 100.0,
            oversampling: 8,
        }
    }
}

/// Interne Sicherheitsmarge unter dem Ceiling (Detektionsungenauigkeit,
/// Rekonstruktion unter variablem Gain).
const SAFETY_DB: f64 = 0.1;
const TAPS_PER_PHASE: usize = 16;

#[derive(Debug, Clone, Copy, Default)]
pub struct LimiterStats {
    /// Kleinster angewendeter Gain (linear).
    pub min_gain: f64,
    /// Anzahl der Ausgabe-Frames mit Gain < 1.
    pub limited_frames: u64,
    pub total_frames: u64,
}

impl LimiterStats {
    pub fn max_reduction_db(&self) -> f64 {
        if self.min_gain >= 1.0 {
            0.0
        } else {
            -lin_to_db(self.min_gain)
        }
    }
    pub fn limited_fraction(&self) -> f64 {
        if self.total_frames == 0 {
            0.0
        } else {
            self.limited_frames as f64 / self.total_frames as f64
        }
    }
}

pub struct TruePeakLimiter {
    channels: usize,
    os: Vec<PeakOversampler>,
    ceiling: f32,
    look: usize,
    det_delay: usize,
    // Verzögerungsleitung je Kanal (Ringpuffer).
    delay: Vec<Vec<f32>>,
    dpos: usize,
    // Gleitendes Minimum: (Index, Wert).
    minq: VecDeque<(u64, f32)>,
    t: u64,
    rel_coeff: f64,
    rel_state: f64,
    // Gleitender Mittelwert.
    avg: Vec<f64>,
    apos: usize,
    asum: f64,
    stats: LimiterStats,
}

impl TruePeakLimiter {
    pub fn new(channels: usize, sample_rate: u32, p: LimiterParams) -> Self {
        let os: Vec<_> = (0..channels)
            .map(|_| PeakOversampler::new(p.oversampling.max(1), TAPS_PER_PHASE))
            .collect();
        let det_delay = os[0].delay();
        let look = ((p.lookahead_ms / 1000.0 * sample_rate as f64).round() as usize).max(1);
        let rel_coeff = (-1.0 / (p.release_ms.max(1.0) / 1000.0 * sample_rate as f64)).exp();
        TruePeakLimiter {
            channels,
            os,
            ceiling: db_to_lin(p.ceiling_dbtp - SAFETY_DB) as f32,
            look,
            det_delay,
            delay: vec![vec![0.0; det_delay + look + 1]; channels],
            dpos: 0,
            minq: VecDeque::with_capacity(look + 2),
            t: 0,
            rel_coeff,
            rel_state: 1.0,
            avg: vec![1.0; look],
            apos: 0,
            asum: look as f64,
            stats: LimiterStats {
                min_gain: 1.0,
                ..Default::default()
            },
        }
    }

    /// Verzögerung des Audiosignals in Samples.
    pub fn latency(&self) -> usize {
        self.det_delay + self.look
    }

    pub fn stats(&self) -> LimiterStats {
        self.stats
    }

    /// Verarbeitet `frames` Frames. `output` erhält ebenso viele Frames,
    /// um [`latency`](Self::latency) verzögert.
    pub fn process(&mut self, input: &[Vec<f32>], output: &mut [Vec<f32>], frames: usize) {
        let ch = self.channels;
        let dlen = self.delay[0].len();
        let w = (self.look + 1) as u64;
        for o in output.iter_mut() {
            o.resize(frames, 0.0);
        }
        for n in 0..frames {
            // 1. Detektion
            let mut peak = 0f32;
            for c in 0..ch {
                let x = input[c][n];
                self.delay[c][self.dpos] = x;
                peak = peak.max(self.os[c].push_peak_above(x, self.ceiling));
                // Stützsamples des Intervalls (t, t+1) mit t = n - D.
                let i_t = (self.dpos + dlen - self.det_delay) % dlen;
                let i_t1 = (i_t + 1) % dlen;
                peak = peak
                    .max(self.delay[c][i_t].abs())
                    .max(self.delay[c][i_t1].abs());
            }
            let r = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };

            // 2. Gleitendes Minimum über L+1 Werte.
            while let Some(&(_, v)) = self.minq.back() {
                if v >= r {
                    self.minq.pop_back();
                } else {
                    break;
                }
            }
            self.minq.push_back((self.t, r));
            while let Some(&(i, _)) = self.minq.front() {
                if i + w <= self.t {
                    self.minq.pop_front();
                } else {
                    break;
                }
            }
            self.t += 1;
            let m = self.minq.front().map_or(1.0, |&(_, v)| v) as f64;

            // 3. Release
            self.rel_state = if m < self.rel_state {
                m
            } else {
                let s = m + (self.rel_state - m) * self.rel_coeff;
                if m - s < 1e-9 { m } else { s }
            };

            // 4. Glättung (Mittelwert über L)
            self.asum += self.rel_state - self.avg[self.apos];
            self.avg[self.apos] = self.rel_state;
            self.apos = (self.apos + 1) % self.look;
            let g = (self.asum / self.look as f64).min(1.0);
            let g32 = g as f32;

            // Ausgabe: Sample von vor D + L Frames.
            let out_idx = (self.dpos + 1) % dlen;
            for c in 0..ch {
                output[c][n] = self.delay[c][out_idx] * g32;
            }
            self.dpos = out_idx;

            self.stats.total_frames += 1;
            if g32 < 1.0 {
                self.stats.limited_frames += 1;
                self.stats.min_gain = self.stats.min_gain.min(g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Läuft das Signal durch den Limiter und entfernt die Latenz.
    fn run(lim: &mut TruePeakLimiter, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let n = input[0].len();
        let lat = lim.latency();
        let mut padded: Vec<Vec<f32>> = input.to_vec();
        for c in padded.iter_mut() {
            c.extend(std::iter::repeat_n(0.0, lat));
        }
        let mut out = vec![Vec::new(); input.len()];
        lim.process(&padded, &mut out, n + lat);
        out.into_iter().map(|c| c[lat..].to_vec()).collect()
    }

    fn ref_true_peak_db(x: &[Vec<f32>]) -> f64 {
        // Unabhängige Referenz: 16× Sinc-Interpolation (siehe testkit).
        const OS: usize = 16;
        const HALF: isize = 48;
        let mut peak = 0f64;
        for ch in x {
            let n = ch.len() as isize;
            for i in 0..n {
                peak = peak.max(ch[i as usize].abs() as f64);
                for p in 1..OS {
                    let t = i as f64 + p as f64 / OS as f64;
                    let mut acc = 0.0;
                    for k in (i - HALF + 1).max(0)..=(i + HALF).min(n - 1) {
                        let d = t - k as f64;
                        let w = 0.5 + 0.5 * (PI * d / (HALF as f64 + 1.0)).cos();
                        acc += ch[k as usize] as f64 * (PI * d).sin() / (PI * d) * w;
                    }
                    peak = peak.max(acc.abs());
                }
            }
        }
        20.0 * peak.log10()
    }

    #[test]
    fn below_ceiling_is_bit_identical_after_delay() {
        let sig: Vec<f32> = (0..20_000)
            .map(|i| (0.5 * (i as f64 * 0.01).sin()) as f32)
            .collect();
        let input = vec![sig.clone(), sig.clone()];
        let mut lim = TruePeakLimiter::new(2, 48_000, LimiterParams::default());
        let out = run(&mut lim, &input);
        assert_eq!(out, input);
        assert_eq!(lim.stats().limited_frames, 0);
    }

    #[test]
    fn intersample_peaks_are_limited() {
        // fs/4-Sinus mit 45° Phase bei 0 dBTP: Samples liegen bei −3 dBFS,
        // ein reiner Sample-Peak-Limiter würde nichts tun.
        let sig: Vec<f32> = (0..24_000)
            .map(|i| (2.0 * PI * 0.25 * i as f64 + PI / 4.0).sin() as f32)
            .collect();
        let input = vec![sig.clone(), sig];
        let mut lim = TruePeakLimiter::new(2, 48_000, LimiterParams::default());
        let out = run(&mut lim, &input);
        let tp = ref_true_peak_db(&out);
        assert!(tp <= -1.0, "True Peak {tp:.3} dBTP");
        assert!(tp > -1.5, "zu stark begrenzt: {tp:.3}");
    }

    /// Weißes Rauschen, tiefpassgefiltert bei 16 kHz (48 kHz Abtastrate).
    ///
    /// Bewusst bandbegrenzt: Inter-Sample-Peaks durch Anteile nahe Nyquist
    /// (> 20 kHz) erkennt kein praxistauglicher Interpolator vollständig
    /// (auch nicht der aus BS.1770). Reale Aufnahmen haben dort kaum Energie.
    fn bandlimited_noise(n: usize, seed: u64) -> Vec<f64> {
        let mut rng = seed;
        let mut next = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            (rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        };
        let white: Vec<f64> = (0..n + 64).map(|_| next()).collect();
        let fc = 16_000.0 / 48_000.0;
        let h: Vec<f64> = (0..65)
            .map(|k| {
                let t = k as f64 - 32.0;
                let sinc = if t == 0.0 {
                    2.0 * fc
                } else {
                    (2.0 * PI * fc * t).sin() / (PI * t)
                };
                sinc * (0.54 - 0.46 * (2.0 * PI * k as f64 / 64.0).cos())
            })
            .collect();
        (0..n)
            .map(|i| {
                h.iter()
                    .enumerate()
                    .map(|(k, c)| c * white[i + k])
                    .sum::<f64>()
                    * 1.6
            })
            .collect()
    }

    #[test]
    fn transient_bursts_never_exceed_ceiling() {
        // Rauschbursts mit schnellem Anstieg, hohe Pegel, Ton bei 9 kHz.
        let n = 48_000;
        let nl = bandlimited_noise(n, 0x1234_5678_9abc_def0);
        let nr = bandlimited_noise(n, 0x0fed_cba9_8765_4321);
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            let burst = if (i / 2400) % 2 == 0 { 2.5 } else { 0.1 };
            let tone = (2.0 * PI * 9_000.0 * i as f64 / 48_000.0).sin();
            l.push((burst * (0.6 * nl[i] + 0.4 * tone)) as f32);
            r.push((burst * nr[i]) as f32);
        }
        for ceiling in [-1.0, -2.0, -0.1] {
            let p = LimiterParams {
                ceiling_dbtp: ceiling,
                ..Default::default()
            };
            let mut lim = TruePeakLimiter::new(2, 48_000, p);
            let out = run(&mut lim, &[l.clone(), r.clone()]);
            let tp = ref_true_peak_db(&out);
            assert!(tp <= ceiling + 0.05, "Ceiling {ceiling}: True Peak {tp:.3}");
            assert!(lim.stats().max_reduction_db() > 6.0);
        }
    }

    #[test]
    fn deterministic() {
        let sig: Vec<f32> = (0..30_000)
            .map(|i| (1.7 * (i as f64 * 0.37).sin() * (i as f64 * 0.001).cos()) as f32)
            .collect();
        let a = run(
            &mut TruePeakLimiter::new(1, 44_100, LimiterParams::default()),
            std::slice::from_ref(&sig),
        );
        let b = run(
            &mut TruePeakLimiter::new(1, 44_100, LimiterParams::default()),
            &[sig],
        );
        assert_eq!(a, b);
    }
}
