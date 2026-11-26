//! TPDF-Dither mit deterministischem PRNG.
//!
//! Der Seed hängt nur von der Kanalnummer ab, nicht von Datei oder
//! Batch-Position: gleiche Eingabe ⇒ gleiche Ausgabe.

pub struct TpdfDither {
    state: Vec<u64>,
    lsb: f32,
}

impl TpdfDither {
    /// `bits`: Zielbittiefe (z. B. 16). Dither-Amplitude ±1 LSB (dreieckförmig).
    pub fn new(channels: usize, bits: u16) -> Self {
        let state = (0..channels)
            .map(|c| 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(c as u64 + 1) | 1)
            .collect();
        TpdfDither {
            state,
            lsb: 1.0 / (1u64 << (bits - 1)) as f32,
        }
    }

    #[inline]
    fn uniform(s: &mut u64) -> f32 {
        let mut x = *s;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *s = x;
        // 24 bit Zufall → [0, 1)
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u32 << 24) as f32
    }

    pub fn apply(&mut self, buf: &mut [Vec<f32>], frames: usize) {
        for (ch, s) in buf.iter_mut().zip(self.state.iter_mut()) {
            for x in &mut ch[..frames] {
                let d = Self::uniform(s) - Self::uniform(s);
                *x += d * self.lsb;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tpdf_statistics() {
        let mut d = TpdfDither::new(1, 16);
        let mut buf = vec![vec![0f32; 200_000]];
        d.apply(&mut buf, 200_000);
        let lsb = 1.0 / 32768.0;
        let max = buf[0].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(max <= lsb);
        let mean = buf[0].iter().map(|&x| x as f64).sum::<f64>() / 200_000.0;
        let var = buf[0].iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / 200_000.0;
        // TPDF ±1 LSB: Varianz = LSB²/6.
        assert!(mean.abs() < 0.01 * lsb as f64);
        let expected = (lsb as f64).powi(2) / 6.0;
        assert!((var / expected - 1.0).abs() < 0.03, "{}", var / expected);
    }

    #[test]
    fn deterministic_and_channels_differ() {
        let mut a = vec![vec![0f32; 100]; 2];
        let mut b = vec![vec![0f32; 100]; 2];
        TpdfDither::new(2, 16).apply(&mut a, 100);
        TpdfDither::new(2, 16).apply(&mut b, 100);
        assert_eq!(a, b);
        assert_ne!(a[0], a[1]);
    }
}
