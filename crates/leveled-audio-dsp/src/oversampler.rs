//! Polyphasen-Interpolator zur True-Peak-Detektion.
//!
//! Liefert pro Eingangssample den Betragsmaximalwert der `factor`
//! interpolierten Zwischenwerte. Der Audiopfad selbst wird nicht
//! hochgetastet; das Oversampling dient nur der Spitzenwerterkennung.

use std::f64::consts::PI;

pub struct PeakOversampler {
    factor: usize,
    taps: usize,
    /// Koeffizienten je Phase, zeitlich umgekehrt (ältestes Sample zuerst).
    phases: Vec<Vec<f32>>,
    /// Doppelt geführter Verlauf, damit das Fenster zusammenhängend ist.
    hist: Vec<f32>,
    pos: usize,
    /// Obere Schranke max_p Σ|h_p|: |y| ≤ bound · max|x| im Fenster.
    bound: f32,
}

/// Modifizierte Besselfunktion 0. Ordnung (Reihenentwicklung) für das Kaiser-Fenster.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-17 * sum {
            break;
        }
    }
    sum
}

impl PeakOversampler {
    /// `factor`: Oversampling-Faktor (z. B. 4), `taps`: Taps pro Phase.
    pub fn new(factor: usize, taps: usize) -> Self {
        assert!(factor >= 1 && taps >= 2);
        let n = factor * taps;
        let beta = 7.0;
        let center = (n as f64 - 1.0) / 2.0;
        let i0b = bessel_i0(beta);
        let mut h: Vec<f64> = (0..n)
            .map(|i| {
                let t = (i as f64 - center) / factor as f64;
                let sinc = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * t).sin() / (PI * t)
                };
                let r = (i as f64 - center) / (center + 0.5);
                let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
                sinc * w
            })
            .collect();
        // Gleichanteil jeder Phase ≈ 1: Gesamtsumme auf `factor` normieren.
        let sum: f64 = h.iter().sum();
        h.iter_mut().for_each(|v| *v *= factor as f64 / sum);

        let phases = (0..factor)
            .map(|p| {
                let mut c: Vec<f32> = (0..taps).map(|k| h[p + k * factor] as f32).collect();
                c.reverse();
                c
            })
            .collect::<Vec<Vec<f32>>>();
        // Kleiner Aufschlag deckt Rundungsfehler der f32-Summation ab.
        let bound = phases
            .iter()
            .map(|c| c.iter().map(|v| v.abs()).sum::<f32>())
            .fold(0f32, f32::max)
            * 1.0001;
        PeakOversampler {
            factor,
            taps,
            phases,
            hist: vec![0.0; 2 * taps],
            pos: 0,
            bound,
        }
    }

    /// Verzögerung (in Eingangssamples), ab der der zurückgegebene Spitzenwert
    /// das Intervall `(n - delay, n - delay + 1)` beschreibt.
    pub fn delay(&self) -> usize {
        self.taps / 2
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    pub fn reset(&mut self) {
        self.hist.iter_mut().for_each(|v| *v = 0.0);
        self.pos = 0;
    }

    /// Schiebt ein Sample ein und liefert max |y| über alle Phasen.
    #[inline]
    pub fn push_peak(&mut self, x: f32) -> f32 {
        self.push(x);
        self.current_peak()
    }

    /// Schiebt ein Sample ein und liefert max |y|, sofern dieser `threshold`
    /// überschreiten *kann*. Sonst wird ohne Interpolation ein Wert
    /// `<= threshold` geliefert. Für Vergleiche mit `threshold` ist das
    /// Ergebnis daher identisch mit [`push_peak`](Self::push_peak), aber
    /// deutlich schneller bei leisen Signalen.
    #[inline]
    pub fn push_peak_above(&mut self, x: f32, threshold: f32) -> f32 {
        self.push(x);
        let t = self.taps;
        let window = &self.hist[self.pos..self.pos + t];
        let max_abs = window.iter().fold(0f32, |m, v| m.max(v.abs()));
        if max_abs * self.bound <= threshold {
            return 0.0;
        }
        self.current_peak()
    }

    #[inline]
    fn push(&mut self, x: f32) {
        let t = self.taps;
        self.hist[self.pos] = x;
        self.hist[self.pos + t] = x;
        self.pos += 1;
        if self.pos == t {
            self.pos = 0;
        }
    }

    #[inline]
    fn current_peak(&self) -> f32 {
        let window = &self.hist[self.pos..self.pos + self.taps];
        let mut peak = 0f32;
        for c in &self.phases {
            peak = peak.max(dot(window, c).abs());
        }
        peak
    }
}

/// Skalarprodukt mit fester Summationsreihenfolge (8 Teilsummen) – schnell
/// und auf allen Plattformen bitidentisch.
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let chunks = a.len() / 8;
    for i in 0..chunks {
        for l in 0..8 {
            acc[l] += a[i * 8 + l] * b[i * 8 + l];
        }
    }
    let mut s = ((acc[0] + acc[1]) + (acc[2] + acc[3])) + ((acc[4] + acc[5]) + (acc[6] + acc[7]));
    for i in chunks * 8..a.len() {
        s += a[i] * b[i];
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eingeschwungener Spitzenwert (Einschwingvorgang am Anfang wird ignoriert).
    fn steady_peak(signal: &[f32], os: &mut PeakOversampler) -> f32 {
        let mut p = 0f32;
        for (i, &x) in signal.iter().enumerate() {
            let v = os.push_peak(x);
            if i > 4 * os.taps {
                p = p.max(v);
            }
        }
        p
    }

    #[test]
    fn dc_gain_is_unity() {
        let mut os = PeakOversampler::new(8, 16);
        let p = steady_peak(&vec![0.5f32; 400], &mut os);
        assert!((p - 0.5).abs() < 0.001, "{p}");
    }

    #[test]
    fn finds_intersample_peaks_across_frequencies() {
        // Sinus der Amplitude 1 in 16 Phasenlagen: 8× erkennt bis 16 kHz
        // (bei 48 kHz) auf 0,1 dB genau, bis 22 kHz auf 0,2 dB.
        for (freq, tol) in [
            (1000.0, 0.01),
            (6000.0, 0.02),
            (12000.0, 0.05),
            (16000.0, 0.1),
            (20000.0, 0.2),
            (22000.0, 0.2),
        ] {
            for ph in 0..16 {
                let phase = ph as f64 * PI / 16.0;
                let sig: Vec<f32> = (0..4800)
                    .map(|i| (2.0 * PI * freq * i as f64 / 48_000.0 + phase).sin() as f32)
                    .collect();
                let mut os = PeakOversampler::new(8, 16);
                let db = 20.0 * (steady_peak(&sig, &mut os) as f64).log10();
                assert!(db.abs() < tol, "{freq} Hz, Phase {ph}: {db:.3} dB");
            }
        }
    }

    #[test]
    fn shortcut_agrees_with_full_computation() {
        let sig: Vec<f32> = (0..20_000)
            .map(|i| {
                let t = i as f64;
                ((t * 0.31).sin() * (t * 0.0007).sin() * 1.2) as f32
            })
            .collect();
        for thr in [0.3f32, 0.8, 1.0] {
            let mut a = PeakOversampler::new(8, 16);
            let mut b = PeakOversampler::new(8, 16);
            for &x in &sig {
                let full = a.push_peak(x);
                let fast = b.push_peak_above(x, thr);
                assert_eq!(full > thr, fast > thr);
                if fast > thr {
                    assert_eq!(full, fast);
                }
            }
        }
    }
}
