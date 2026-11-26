//! Hochpass 2. Ordnung (Butterworth, RBJ-Biquad) für den Pre-Chain-Slot
//! (PLAN 3.13). Zustand in `f64`, je Kanal, ohne Latenz.

#[derive(Debug, Clone)]
pub struct Highpass {
    b: [f64; 3],
    a: [f64; 2],
    /// Je Kanal: x[n-1], x[n-2], y[n-1], y[n-2].
    state: Vec<[f64; 4]>,
}

impl Highpass {
    pub fn new(channels: usize, sample_rate: u32, cutoff_hz: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * cutoff_hz / sample_rate as f64;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Highpass {
            b: [
                (1.0 + cos) / 2.0 / a0,
                -(1.0 + cos) / a0,
                (1.0 + cos) / 2.0 / a0,
            ],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            state: vec![[0.0; 4]; channels],
        }
    }

    /// Filtert `frames` Samples je Kanal an Ort und Stelle.
    pub fn process(&mut self, buf: &mut [Vec<f32>], frames: usize) {
        let [b0, b1, b2] = self.b;
        let [a1, a2] = self.a;
        for (ch, st) in buf.iter_mut().zip(self.state.iter_mut()) {
            let [mut x1, mut x2, mut y1, mut y2] = *st;
            for v in &mut ch[..frames] {
                let x = *v as f64;
                let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
                x2 = x1;
                x1 = x;
                y2 = y1;
                y1 = y;
                *v = y as f32;
            }
            *st = [x1, x2, y1, y2];
        }
    }

    pub fn reset(&mut self) {
        self.state.iter_mut().for_each(|s| *s = [0.0; 4]);
    }

    /// Betragsfrequenzgang in dB bei `hz`.
    pub fn response_db(&self, sample_rate: u32, hz: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * hz / sample_rate as f64;
        let z = |k: f64| ((-k * w).cos(), (-k * w).sin());
        let (c1, s1) = z(1.0);
        let (c2, s2) = z(2.0);
        let nr = self.b[0] + self.b[1] * c1 + self.b[2] * c2;
        let ni = self.b[1] * s1 + self.b[2] * s2;
        let dr = 1.0 + self.a[0] * c1 + self.a[1] * c2;
        let di = self.a[0] * s1 + self.a[1] * s2;
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn butterworth_response() {
        let h = Highpass::new(1, 48_000, 70.0);
        assert!((h.response_db(48_000, 70.0) + 3.01).abs() < 0.05);
        assert!(h.response_db(48_000, 35.0) < -11.0);
        assert!(h.response_db(48_000, 85.0) > -2.0);
        assert!(h.response_db(48_000, 1000.0).abs() < 0.05);
    }

    #[test]
    fn removes_dc_and_matches_response_on_sine() {
        let rate = 44_100;
        let mut h = Highpass::new(1, rate, 70.0);
        let n = rate as usize;
        let mut x = vec![
            (0..n)
                .map(|i| {
                    0.5 + 0.3
                        * (2.0 * std::f64::consts::PI * 35.0 * i as f64 / rate as f64).sin() as f32
                })
                .collect::<Vec<f32>>(),
        ];
        h.process(&mut x, n);
        // Ganze Perioden (35 Hz bei 44,1 kHz: 1260 Samples), sonst verfälscht
        // der Sinusrest den Mittelwert.
        let tail = &x[0][n - 17 * 1260..];
        let mean = tail.iter().map(|v| *v as f64).sum::<f64>() / tail.len() as f64;
        let rms =
            (tail.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        assert!(mean.abs() < 1e-4, "DC {mean}");
        let expected = 0.3 / 2f64.sqrt() * 10f64.powf(h.response_db(rate, 35.0) / 20.0);
        assert!((rms / expected - 1.0).abs() < 0.02, "{rms} vs {expected}");
    }
}
