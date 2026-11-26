//! Lautheitsmessung (Integrated, LRA, True Peak, Sample Peak) über `ebur128`.
//!
//! Läuft im Histogramm-Modus, damit der Speicherbedarf auch bei
//! mehrstündigen Dateien konstant bleibt.

use ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
#[error("Lautheitsmessung: {0}")]
pub struct LoudnessError(String);

impl From<ebur128::Error> for LoudnessError {
    fn from(e: ebur128::Error) -> Self {
        LoudnessError(format!("{e:?}"))
    }
}

/// Messergebnis. `None` bei Werten, die nicht definiert sind (z. B. Stille).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoudnessStats {
    /// Integrierte Lautheit in LUFS.
    pub integrated_lufs: Option<f64>,
    /// Loudness Range in LU.
    pub lra_lu: Option<f64>,
    /// Maximaler True Peak über alle Kanäle in dBTP.
    pub true_peak_dbtp: Option<f64>,
    /// Maximaler Sample Peak über alle Kanäle in dBFS.
    pub sample_peak_dbfs: Option<f64>,
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

fn lin_to_db(v: f64) -> Option<f64> {
    (v > 0.0).then(|| 20.0 * v.log10())
}

pub struct LoudnessMeter {
    inner: EbuR128,
    channels: usize,
}

impl LoudnessMeter {
    pub fn new(channels: usize, sample_rate: u32) -> Result<Self, LoudnessError> {
        let mode = Mode::I | Mode::LRA | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK | Mode::HISTOGRAM;
        let inner = EbuR128::new(channels as u32, sample_rate, mode)?;
        Ok(LoudnessMeter { inner, channels })
    }

    /// Fügt `frames` Frames aus planaren Kanalpuffern hinzu.
    pub fn add(&mut self, channels: &[Vec<f32>], frames: usize) -> Result<(), LoudnessError> {
        if frames == 0 {
            return Ok(());
        }
        let slices: Vec<&[f32]> = channels.iter().map(|c| &c[..frames]).collect();
        self.inner.add_frames_planar_f32(&slices)?;
        Ok(())
    }

    pub fn stats(&self) -> Result<LoudnessStats, LoudnessError> {
        let mut tp = 0f64;
        let mut sp = 0f64;
        for c in 0..self.channels as u32 {
            tp = tp.max(self.inner.true_peak(c)?);
            sp = sp.max(self.inner.sample_peak(c)?);
        }
        Ok(LoudnessStats {
            integrated_lufs: finite(self.inner.loudness_global()?),
            lra_lu: finite(self.inner.loudness_range()?),
            true_peak_dbtp: lin_to_db(tp),
            sample_peak_dbfs: lin_to_db(sp),
        })
    }

    /// Gemeinsame integrierte Lautheit mehrerer Messungen (Album-Modus).
    pub fn integrated_multiple<'a>(
        meters: impl Iterator<Item = &'a LoudnessMeter>,
    ) -> Result<Option<f64>, LoudnessError> {
        Ok(finite(EbuR128::loudness_global_multiple(
            meters.map(|m| &m.inner),
        )?))
    }
}

#[cfg(test)]
mod tests {
    //! Verifikation gegen EBU Tech 3341 (Fälle 1–5) und Tech 3342 (LRA,
    //! Fälle 1–4). Die Signale sind synthetisch nach der Spezifikation erzeugt.
    use super::*;
    use leveled_audio_testkit::{Planar, sine, sine_phase, tone_sequence};

    fn measure(data: &Planar, rate: u32) -> LoudnessStats {
        let mut m = LoudnessMeter::new(data.len(), rate).unwrap();
        // In Blöcken zuführen wie die Pipeline.
        let n = data[0].len();
        let mut pos = 0;
        while pos < n {
            let k = 4096.min(n - pos);
            let block: Vec<Vec<f32>> = data.iter().map(|c| c[pos..pos + k].to_vec()).collect();
            m.add(&block, k).unwrap();
            pos += k;
        }
        m.stats().unwrap()
    }

    fn assert_close(v: Option<f64>, expected: f64, tol: f64, what: &str) {
        let v = v.unwrap_or_else(|| panic!("{what}: kein Wert"));
        assert!(
            (v - expected).abs() <= tol,
            "{what}: {v:.3}, erwartet {expected} ± {tol}"
        );
    }

    const RATES: [u32; 3] = [44_100, 48_000, 96_000];

    #[test]
    fn tech3341_case1_and_2() {
        for rate in RATES {
            let s = measure(&sine(rate, 2, 1000.0, -23.0, 20.0), rate);
            assert_close(s.integrated_lufs, -23.0, 0.1, "3341/1 I");
            let s = measure(&sine(rate, 2, 1000.0, -33.0, 20.0), rate);
            assert_close(s.integrated_lufs, -33.0, 0.1, "3341/2 I");
        }
    }

    #[test]
    fn tech3341_case3_to_5() {
        for rate in RATES {
            let c3 = tone_sequence(
                rate,
                2,
                1000.0,
                &[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)],
            );
            assert_close(measure(&c3, rate).integrated_lufs, -23.0, 0.1, "3341/3 I");
            let c4 = tone_sequence(
                rate,
                2,
                1000.0,
                &[
                    (-72.0, 10.0),
                    (-36.0, 10.0),
                    (-23.0, 60.0),
                    (-36.0, 10.0),
                    (-72.0, 10.0),
                ],
            );
            assert_close(measure(&c4, rate).integrated_lufs, -23.0, 0.1, "3341/4 I");
            let c5 = tone_sequence(
                rate,
                2,
                1000.0,
                &[(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)],
            );
            assert_close(measure(&c5, rate).integrated_lufs, -23.0, 0.1, "3341/5 I");
        }
    }

    #[test]
    fn tech3342_lra() {
        let rate = 48_000;
        let cases: [(&[(f64, f64)], f64); 4] = [
            (&[(-20.0, 20.0), (-30.0, 20.0)], 10.0),
            (&[(-20.0, 20.0), (-15.0, 20.0)], 5.0),
            (&[(-40.0, 20.0), (-20.0, 20.0)], 20.0),
            (
                &[
                    (-50.0, 20.0),
                    (-35.0, 20.0),
                    (-20.0, 20.0),
                    (-35.0, 20.0),
                    (-50.0, 20.0),
                ],
                15.0,
            ),
        ];
        for (i, (seg, lra)) in cases.iter().enumerate() {
            let s = measure(&tone_sequence(rate, 2, 1000.0, seg), rate);
            assert_close(s.lra_lu, *lra, 1.0, &format!("3342/{}", i + 1));
        }
    }

    #[test]
    fn true_peak_of_intersample_sine() {
        // fs/4 mit 45° Phase: Sample Peak −3,01 dB unter dem wahren Peak.
        let s = sine_phase(48_000, 2, 12_000.0, -6.0, 2.0, std::f64::consts::FRAC_PI_4);
        let st = measure(&s, 48_000);
        assert_close(st.sample_peak_dbfs, -9.01, 0.05, "Sample Peak");
        // Tech-3341-Toleranz für True Peak: +0,2 / −0,4 dB.
        let tp = st.true_peak_dbtp.unwrap();
        assert!((-6.4..=-5.8).contains(&tp), "True Peak {tp}");
    }

    #[test]
    fn mono_is_one_channel() {
        // Mono-Sinus −23 dBFS: nur ein Kanal ⇒ 3 dB unter Stereo.
        let s = measure(&sine(48_000, 1, 1000.0, -23.0, 20.0), 48_000);
        assert_close(s.integrated_lufs, -26.0, 0.1, "Mono I");
    }

    #[test]
    fn silence_has_no_loudness() {
        let s = measure(&vec![vec![0.0; 48_000 * 3]; 2], 48_000);
        assert_eq!(s.integrated_lufs, None);
        assert_eq!(s.true_peak_dbtp, None);
    }

    #[test]
    fn album_loudness_combines() {
        let rate = 48_000;
        let a = sine(rate, 2, 1000.0, -20.0, 10.0);
        let b = sine(rate, 2, 1000.0, -30.0, 10.0);
        let mut ma = LoudnessMeter::new(2, rate).unwrap();
        ma.add(&a, a[0].len()).unwrap();
        let mut mb = LoudnessMeter::new(2, rate).unwrap();
        mb.add(&b, b[0].len()).unwrap();
        let album = LoudnessMeter::integrated_multiple([&ma, &mb].into_iter())
            .unwrap()
            .unwrap();
        // Energiemittel von −20 und −30 LUFS ≈ −22,6 LUFS (beide über dem relativen Gate).
        assert!((album + 22.6).abs() < 0.1, "{album}");
    }
}
