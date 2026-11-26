//! Anwendung der offline berechneten Leveler-Kurve im Strom.

use leveled_audio_analysis::LevelerCurve;

pub(crate) struct LevelerStage {
    curve: LevelerCurve,
    /// Linearer Gain je Stützstelle.
    lin: Vec<f32>,
}

impl LevelerStage {
    pub fn new(curve: LevelerCurve) -> Self {
        let lin = curve.gain_db.iter().map(|g| 10f32.powf(g / 20.0)).collect();
        LevelerStage { curve, lin }
    }

    pub fn curve(&self) -> &LevelerCurve {
        &self.curve
    }

    /// Wendet die Kurve auf `n` Frames an; `first` ist der Eingangs-Sampleindex
    /// des ersten Frames (negativ während des Latenz-Vorlaufs).
    pub fn apply(&self, buf: &mut [Vec<f32>], n: usize, first: i64) {
        let hop = self.curve.hop as i64;
        let inv = 1.0 / hop as f32;
        let last = self.lin.len().saturating_sub(1);
        for i in 0..n {
            let u = first + i as i64 - self.curve.first_center;
            let g = if self.lin.is_empty() {
                1.0
            } else if u <= 0 {
                self.lin[0]
            } else {
                let k = (u / hop) as usize;
                if k >= last {
                    self.lin[last]
                } else {
                    let frac = (u % hop) as f32 * inv;
                    self.lin[k] + frac * (self.lin[k + 1] - self.lin[k])
                }
            };
            for c in buf.iter_mut() {
                c[i] *= g;
            }
        }
    }
}
