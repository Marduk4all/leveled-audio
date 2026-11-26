//! Frame-Features aus den STFT-Frames von Pass 1.
//!
//! Alle Leistungen sind auf „mittlere Leistung pro Sample“ normiert
//! (Parseval mit Hann-Fenster), sodass `e_db` direkt in dBFS (RMS)
//! vergleichbar ist. Kanäle werden über die **Leistung** gemittelt, nicht über
//! ein Mid-Signal – robust gegen Phasenumkehr zwischen L und R.

use leveled_audio_dsp::Complex32;

/// Kompakte Analysedaten je Frame (≈ 16 Byte pro Frame).
#[derive(Debug, Clone, Default)]
pub struct FrameFeatures {
    pub sample_rate: u32,
    pub fft_size: usize,
    pub hop: usize,
    /// Bandleistung 100–4000 Hz in dBFS.
    pub e_db: Vec<f32>,
    /// Spektrale Flachheit 300–6000 Hz (0 = tonal, 1 = Rauschen).
    pub flatness: Vec<f32>,
    /// Positiver spektraler Flux (mittlerer dB-Anstieg je Bin, 100–8000 Hz).
    pub flux: Vec<f32>,
    /// Mittel der log. Bin-Leistungen 100–8000 Hz (dB). Jeder Bin zählt gleich;
    /// dient der Profil-Frame-Auswahl ohne Auswahl-Bias in einzelnen Bändern.
    pub log_mean_db: Vec<f32>,
    /// Anteil unveränderter Folgesamples der neuen Hop-Samples (digitale Stille).
    pub flat_frac: Vec<f32>,
}

impl FrameFeatures {
    pub fn len(&self) -> usize {
        self.e_db.len()
    }

    pub fn is_empty(&self) -> bool {
        self.e_db.is_empty()
    }

    pub fn hop_seconds(&self) -> f64 {
        self.hop as f64 / self.sample_rate as f64
    }

    /// Frames für eine Dauer in Sekunden (mindestens 1).
    pub fn frames_for(&self, secs: f64) -> usize {
        ((secs / self.hop_seconds()).round() as usize).max(1)
    }
}

/// Umrechnung Bin-Leistungssumme → mittlere Leistung pro Sample.
/// Für reelle Signale zählt jeder Bin außer DC/Nyquist doppelt; Σw² = 3N/8.
pub fn power_scale(fft_size: usize) -> f64 {
    2.0 / (fft_size as f64 * fft_size as f64 * 3.0 / 8.0)
}

fn bin_range(lo_hz: f64, hi_hz: f64, rate: u32, n: usize) -> (usize, usize) {
    let bins = n / 2 + 1;
    let k = |hz: f64| ((hz * n as f64 / rate as f64).round() as usize).min(bins - 1);
    let (a, b) = (k(lo_hz).max(1), k(hi_hz));
    (a, b.max(a + 1))
}

/// Pegel (dBFS) eines Leistungsspektrums im selben Band wie `e_db`
/// (100–4000 Hz, Kanalmittel). Damit lassen sich Profile mit Pausenpegeln vergleichen.
pub fn band_level_db(power: &[Vec<f32>], sample_rate: u32, fft_size: usize) -> f32 {
    let nyq = sample_rate as f64 / 2.0;
    let (a, b) = bin_range(100.0, 4000.0f64.min(nyq), sample_rate, fft_size);
    let ch = power.len().max(1) as f64;
    let sum: f64 = power
        .iter()
        .map(|c| c[a..b].iter().map(|v| *v as f64).sum::<f64>())
        .sum::<f64>()
        / ch;
    (10.0 * (sum * power_scale(fft_size) + 1e-20).log10()) as f32
}

pub struct FeatureExtractor {
    scale: f64,
    e_band: (usize, usize),
    flat_band: (usize, usize),
    flux_band: (usize, usize),
    pow: Vec<f32>,
    prev_db: Vec<f32>,
    have_prev: bool,
    last_sample: Vec<f32>,
    pub features: FrameFeatures,
}

impl FeatureExtractor {
    pub fn new(sample_rate: u32, fft_size: usize, channels: usize) -> Self {
        let bins = fft_size / 2 + 1;
        // Bandgrenzen nicht über Nyquist hinaus.
        let nyq = sample_rate as f64 / 2.0;
        FeatureExtractor {
            scale: power_scale(fft_size),
            e_band: bin_range(100.0, 4000.0f64.min(nyq), sample_rate, fft_size),
            flat_band: bin_range(300.0, 6000.0f64.min(nyq), sample_rate, fft_size),
            flux_band: bin_range(100.0, 8000.0f64.min(nyq), sample_rate, fft_size),
            pow: vec![0.0; bins],
            prev_db: vec![0.0; bins],
            have_prev: false,
            last_sample: vec![f32::NAN; channels],
            features: FrameFeatures {
                sample_rate,
                fft_size,
                hop: fft_size / 4,
                ..Default::default()
            },
        }
    }

    /// Wertet einen STFT-Frame aus (`time`: ungefensterte Frame-Samples je Kanal).
    pub fn push(&mut self, spectra: &[Vec<Complex32>], time: &[Vec<f32>]) {
        let ch = spectra.len() as f32;
        for (k, p) in self.pow.iter_mut().enumerate() {
            *p = spectra.iter().map(|s| s[k].norm_sqr()).sum::<f32>() / ch;
        }
        let f = &mut self.features;

        let (a, b) = self.e_band;
        let e: f64 = self.pow[a..b].iter().map(|v| *v as f64).sum::<f64>() * self.scale;
        f.e_db.push((10.0 * (e + 1e-20).log10()) as f32);

        let (a, b) = self.flat_band;
        let tiny = 1e-20f64;
        let n = (b - a) as f64;
        let mut log_sum = 0f64;
        let mut lin_sum = 0f64;
        for &v in &self.pow[a..b] {
            let v = v as f64 + tiny;
            log_sum += v.ln();
            lin_sum += v;
        }
        f.flatness
            .push(((log_sum / n).exp() / (lin_sum / n)).clamp(0.0, 1.0) as f32);

        let (a, b) = self.flux_band;
        let mut flux = 0f32;
        let mut log_sum = 0f64;
        for k in a..b {
            let db = 10.0 * (self.pow[k] + 1e-20).log10();
            if self.have_prev {
                flux += (db - self.prev_db[k]).max(0.0);
            }
            self.prev_db[k] = db;
            log_sum += db as f64;
        }
        self.have_prev = true;
        f.flux.push(flux / (b - a) as f32);
        f.log_mean_db.push((log_sum / (b - a) as f64) as f32);

        // Digitale Stille: Folgesamples der neuen Hop-Samples unverändert.
        let hop = f.hop;
        let mut same = 0usize;
        let mut total = 0usize;
        for (c, t) in time.iter().enumerate() {
            let fresh = &t[t.len() - hop..];
            let mut prev = self.last_sample[c];
            for &x in fresh {
                if x == prev {
                    same += 1;
                }
                prev = x;
                total += 1;
            }
            self.last_sample[c] = prev;
        }
        f.flat_frac.push(same as f32 / total.max(1) as f32);
    }

    pub fn finish(self) -> FrameFeatures {
        self.features
    }
}
