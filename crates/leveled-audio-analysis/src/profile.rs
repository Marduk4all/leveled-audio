//! Automatische Rauschprofilsuche, Profilaufbau, Konfidenz und Profilformat.

use crate::features::{FrameFeatures, power_scale};
use crate::vad::FrameClass;
use leveled_audio_dsp::Complex32;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileSource {
    /// Aus automatisch gefundenen Pausen.
    Auto,
    /// Minimum-Statistics-Fallback (keine verwertbaren Pausen).
    Minstat,
    /// Aus Datei importiert.
    Imported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileStatistic {
    Mean,
    Trimmed,
    Median,
}

/// Rauschprofil. Gespeichert wird die Leistung je Bin in dB (lesbar, verlustfrei
/// über JSON); intern wird mit linearer Leistung gerechnet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseProfile {
    pub format_version: u32,
    pub sample_rate: u32,
    pub fft_size: usize,
    pub window: String,
    pub source: ProfileSource,
    pub statistic: ProfileStatistic,
    pub confidence: f64,
    pub duration_s: f64,
    pub pauses_used: usize,
    /// Leistung je Kanal und Bin in dB (Rohwert |X|² des Hann-gefensterten Frames).
    pub bins_db: Vec<Vec<f32>>,
}

pub const PROFILE_FORMAT_VERSION: u32 = 1;

fn to_db(p: f32) -> f32 {
    10.0 * p.max(1e-30).log10()
}

fn from_db(d: f32) -> f32 {
    10f32.powf(d / 10.0)
}

impl NoiseProfile {
    pub fn from_power(
        power: &[Vec<f32>],
        sample_rate: u32,
        fft_size: usize,
        source: ProfileSource,
        statistic: ProfileStatistic,
    ) -> Self {
        NoiseProfile {
            format_version: PROFILE_FORMAT_VERSION,
            sample_rate,
            fft_size,
            window: "hann".into(),
            source,
            statistic,
            confidence: 0.0,
            duration_s: 0.0,
            pauses_used: 0,
            bins_db: power
                .iter()
                .map(|c| c.iter().map(|&p| to_db(p)).collect())
                .collect(),
        }
    }

    pub fn channels(&self) -> usize {
        self.bins_db.len()
    }

    /// Mittlerer Rauschpegel (RMS, dBFS) über alle Kanäle.
    pub fn level_dbfs(&self) -> f64 {
        let scale = power_scale(self.fft_size);
        let per_ch: Vec<f64> = self
            .bins_db
            .iter()
            .map(|c| {
                let last = c.len() - 1;
                c.iter()
                    .enumerate()
                    .map(|(k, &d)| {
                        let w = if k == 0 || k == last { 0.5 } else { 1.0 };
                        w * from_db(d) as f64
                    })
                    .sum::<f64>()
                    * scale
            })
            .collect();
        let mean = per_ch.iter().sum::<f64>() / per_ch.len().max(1) as f64;
        10.0 * (mean + 1e-30).log10()
    }

    /// Leistung je Bin für eine Zielkonfiguration. Bei anderer Samplerate oder
    /// FFT-Größe wird über die Frequenz interpoliert (linear in dB) und die
    /// Rohleistung auf die andere Fensterlänge umgerechnet. Mono → Stereo:
    /// Profil für beide Kanäle; Stereo → Mono: Mittelwert.
    pub fn power_for(&self, sample_rate: u32, fft_size: usize, channels: usize) -> Vec<Vec<f32>> {
        let src_bins = self.fft_size / 2 + 1;
        let dst_bins = fft_size / 2 + 1;
        // Rohleistung skaliert mit der Fensterenergie (∝ N) bei gleicher PSD.
        let level_shift = 10.0 * ((fft_size as f64 / self.fft_size as f64).log10()) as f32;
        let mapped: Vec<Vec<f32>> = self
            .bins_db
            .iter()
            .map(|c| {
                (0..dst_bins)
                    .map(|k| {
                        let hz = k as f64 * sample_rate as f64 / fft_size as f64;
                        let x = hz * self.fft_size as f64 / self.sample_rate as f64;
                        let i = (x.floor() as usize).min(src_bins - 1);
                        let j = (i + 1).min(src_bins - 1);
                        let t = (x - i as f64).clamp(0.0, 1.0) as f32;
                        let d = c[i] * (1.0 - t) + c[j] * t;
                        from_db(d + level_shift)
                    })
                    .collect()
            })
            .collect();
        match (mapped.len(), channels) {
            (a, b) if a == b => mapped,
            (1, _) => vec![mapped[0].clone(); channels],
            (_, 1) => {
                let n = mapped.len() as f32;
                vec![
                    (0..dst_bins)
                        .map(|k| mapped.iter().map(|c| c[k]).sum::<f32>() / n)
                        .collect(),
                ]
            }
            _ => (0..channels)
                .map(|c| mapped[c % mapped.len()].clone())
                .collect(),
        }
    }
}

// ------------------------------------------------------------ Frame-Auswahl

#[derive(Debug, Clone, Copy)]
pub struct ProfileSearchParams {
    pub min_duration_s: f64,
    pub target_duration_s: f64,
    pub max_duration_s: f64,
    /// Zusätzlicher Abstand zu Sprache (zum VAD-Hangover von 150 ms hinzu).
    pub speech_margin_ms: f64,
    /// Mindestabstand zu digitaler Stille (Fades, Schnittkanten).
    pub silence_margin_ms: f64,
    /// Untergrenze für die zulässige Streuung von `e_db` über ±5 Frames.
    /// Die tatsächliche Schwelle ist adaptiv: typische Streuung in Pausen + 1 dB.
    pub stationarity_db: f32,
    pub min_run_ms: f64,
    /// Die leisesten Kandidaten unterhalb dieses Perzentils werden verworfen.
    pub percentile_lo: f64,
    /// Mindestens dieser Anteil des Kandidatenvorrats wird genommen …
    pub min_fraction: f64,
    /// … aber nie Frames oberhalb dieses Perzentils.
    pub percentile_max: f64,
    /// Randausschluss: min(edge_s, edge_fraction · Dauer).
    pub edge_s: f64,
    pub edge_fraction: f64,
}

impl Default for ProfileSearchParams {
    fn default() -> Self {
        ProfileSearchParams {
            min_duration_s: 0.3,
            target_duration_s: 2.0,
            max_duration_s: 10.0,
            speech_margin_ms: 40.0,
            silence_margin_ms: 300.0,
            stationarity_db: 1.5,
            min_run_ms: 85.0,
            percentile_lo: 0.03,
            min_fraction: 0.30,
            percentile_max: 0.80,
            edge_s: 0.5,
            edge_fraction: 0.05,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Ausgewählte Frames (aufsteigend).
    pub frames: Vec<usize>,
    /// Anzahl verschiedener Pausen, aus denen Frames stammen.
    pub pauses: usize,
    /// Median `e_db` der Sprachframes minus Median der ausgewählten Frames.
    pub separation_db: Option<f64>,
}

impl Selection {
    /// Zusammenhängende Frame-Bereiche `[start, end)` (für effizientes Lesen).
    pub fn runs(&self) -> Vec<(usize, usize)> {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for &k in &self.frames {
            match runs.last_mut() {
                Some(r) if r.1 == k => r.1 = k + 1,
                _ => runs.push((k, k + 1)),
            }
        }
        runs
    }
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

fn percentile(sorted: &[f32], q: f64) -> f32 {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// Abstand jedes Frames zum nächsten Frame der Klasse `cls` (in Frames).
fn distance_to(classes: &[FrameClass], cls: FrameClass) -> Vec<usize> {
    let n = classes.len();
    let mut d = vec![usize::MAX; n];
    let mut last = None;
    for k in 0..n {
        if classes[k] == cls {
            last = Some(k);
        }
        if let Some(l) = last {
            d[k] = k - l;
        }
    }
    last = None;
    for k in (0..n).rev() {
        if classes[k] == cls {
            last = Some(k);
        }
        if let Some(l) = last {
            d[k] = d[k].min(l - k);
        }
    }
    d
}

pub fn select_frames(
    f: &FrameFeatures,
    classes: &[FrameClass],
    p: &ProfileSearchParams,
) -> Selection {
    let n = f.len();
    if n == 0 {
        return Selection::default();
    }
    let ms = |m: f64| f.frames_for(m / 1000.0);
    let duration = n as f64 * f.hop_seconds();
    let edge = f.frames_for(p.edge_s.min(p.edge_fraction * duration));
    let d_speech = distance_to(classes, FrameClass::Speech);
    let d_silence = distance_to(classes, FrameClass::Silence);
    let speech_margin = ms(p.speech_margin_ms);
    let silence_margin = ms(p.silence_margin_ms);

    // Flux-Schwelle aus der Verteilung in Pausen (Median + 3·MAD).
    let mut pause_flux: Vec<f32> = (0..n)
        .filter(|&k| classes[k] == FrameClass::Pause)
        .map(|k| f.flux[k])
        .collect();
    let flux_med = median(&mut pause_flux).unwrap_or(0.0);
    let mut dev: Vec<f32> = pause_flux.iter().map(|v| (v - flux_med).abs()).collect();
    let flux_mad = median(&mut dev).unwrap_or(0.0);
    let flux_max = flux_med + 3.0 * flux_mad.max(0.1);

    let local_std = |k: usize| -> f32 {
        let lo = k.saturating_sub(5);
        let hi = (k + 6).min(n);
        let w = &f.e_db[lo..hi];
        let m = w.iter().sum::<f32>() / w.len() as f32;
        (w.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / w.len() as f32).sqrt()
    };

    // Stationarität adaptiv: echtes Rauschen auf niedrigem Pegel schwankt je
    // nach Spektrum mehrere dB von Frame zu Frame. Ausgeschlossen werden Frames,
    // die deutlich unruhiger sind als die typische Pause.
    let stds: Vec<f32> = (0..n).map(local_std).collect();
    let mut pause_std: Vec<f32> = (0..n)
        .filter(|&k| classes[k] == FrameClass::Pause)
        .map(|k| stds[k])
        .collect();
    let std_max =
        median(&mut pause_std).map_or(p.stationarity_db, |m| (m + 1.0).max(p.stationarity_db));

    let cand: Vec<bool> = (0..n)
        .map(|k| {
            classes[k] == FrameClass::Pause
                && k >= edge
                && k + edge < n
                && d_speech[k] >= speech_margin
                && d_silence[k] >= silence_margin
                && f.flux[k] <= flux_max
                && stds[k] < std_max
        })
        .collect();

    // Nur ausreichend lange Läufe; jedem Lauf eine Pausen-ID geben.
    let min_run = ms(p.min_run_ms);
    let mut pause_id = vec![usize::MAX; n];
    let mut next_id = 0;
    let mut k = 0;
    while k < n {
        if !cand[k] {
            k += 1;
            continue;
        }
        let s = k;
        while k < n && cand[k] {
            k += 1;
        }
        if k - s >= min_run {
            pause_id[s..k].iter_mut().for_each(|v| *v = next_id);
            next_id += 1;
        }
    }
    let mut pool: Vec<usize> = (0..n).filter(|&k| pause_id[k] != usize::MAX).collect();
    if pool.is_empty() {
        return Selection::default();
    }

    // Perzentil-Auswahl: das absolute Minimum (unter P3) verwerfen, dann von
    // leise nach laut sammeln – mindestens `min_fraction` des Vorrats bzw. bis
    // zur Zieldauer, nie über P80 hinaus und höchstens bis zur Maximaldauer.
    //
    // Sortiert wird nach dem Mittel der log. Bin-Leistungen, nicht nach der
    // Bandenergie: Bei tieffrequentem Rauschen bestimmen sonst wenige Bass-Bins
    // die Energie, und „leise“ Frames wären vor allem solche mit zufällig
    // schwachem Bass – das Profil läge dort systematisch zu niedrig.
    let key = &f.log_mean_db;
    let mut energies: Vec<f32> = pool.iter().map(|&k| key[k]).collect();
    energies.sort_by(f32::total_cmp);
    let lo = percentile(&energies, p.percentile_lo);
    let hi = percentile(&energies, p.percentile_max);
    pool.retain(|&k| key[k] >= lo && key[k] <= hi);
    pool.sort_by(|&a, &b| key[a].total_cmp(&key[b]).then(a.cmp(&b)));
    let wanted = ((energies.len() as f64 * p.min_fraction).ceil() as usize)
        .max(f.frames_for(p.target_duration_s))
        .min(f.frames_for(p.max_duration_s));
    pool.truncate(wanted);
    pool.sort_unstable();

    let mut ids: Vec<usize> = pool.iter().map(|&k| pause_id[k]).collect();
    ids.dedup();
    ids.sort_unstable();
    ids.dedup();

    let mut sel_e: Vec<f32> = pool.iter().map(|&k| f.e_db[k]).collect();
    let mut speech_e: Vec<f32> = (0..n)
        .filter(|&k| classes[k] == FrameClass::Speech)
        .map(|k| f.e_db[k])
        .collect();
    let separation = match (median(&mut speech_e), median(&mut sel_e)) {
        (Some(s), Some(q)) => Some((s - q) as f64),
        _ => None,
    };
    Selection {
        frames: pool,
        pauses: ids.len(),
        separation_db: separation,
    }
}

// ------------------------------------------------------------ Profilaufbau

/// Sammelt Leistungsspektren der ausgewählten Frames und bildet das Profil.
pub struct ProfileBuilder {
    bins: usize,
    /// [Kanal][Frame][Bin]
    frames: Vec<Vec<Vec<f32>>>,
}

impl ProfileBuilder {
    pub fn new(channels: usize, bins: usize) -> Self {
        ProfileBuilder {
            bins,
            frames: vec![Vec::new(); channels],
        }
    }

    pub fn push(&mut self, spectra: &[Vec<Complex32>]) {
        for (c, s) in spectra.iter().enumerate() {
            self.frames[c].push(s.iter().map(|v| v.norm_sqr()).collect());
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frames[0].len()
    }

    /// Profil (Leistung) und Stationaritätsmaß (Median-Std der Bin-Pegel in dB,
    /// Bins 100 Hz … 8 kHz).
    pub fn finish(
        &self,
        statistic: ProfileStatistic,
        sample_rate: u32,
        fft_size: usize,
    ) -> (Vec<Vec<f32>>, f64) {
        let n = self.frame_count();
        let mut col = vec![0f32; n];
        let power: Vec<Vec<f32>> = self
            .frames
            .iter()
            .map(|fr| {
                (0..self.bins)
                    .map(|k| {
                        for (i, f) in fr.iter().enumerate() {
                            col[i] = f[k];
                        }
                        aggregate(&mut col, statistic)
                    })
                    .collect()
            })
            .collect();

        let k_lo = (100.0 * fft_size as f64 / sample_rate as f64).ceil() as usize;
        let k_hi = ((8000.0 * fft_size as f64 / sample_rate as f64) as usize).min(self.bins - 1);
        let mut stds = Vec::new();
        for fr in &self.frames {
            for k in k_lo.max(1)..=k_hi {
                let vals: Vec<f64> = fr
                    .iter()
                    .map(|f| 10.0 * (f[k] as f64 + 1e-30).log10())
                    .collect();
                let m = vals.iter().sum::<f64>() / n as f64;
                let v = vals.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n.max(2) as f64;
                stds.push(v.sqrt() as f32);
            }
        }
        let stat = median(&mut stds).unwrap_or(0.0) as f64;
        (power, stat)
    }
}

fn aggregate(col: &mut [f32], statistic: ProfileStatistic) -> f32 {
    let n = col.len();
    if n == 0 {
        return 0.0;
    }
    match statistic {
        ProfileStatistic::Mean => col.iter().map(|v| *v as f64).sum::<f64>() as f32 / n as f32,
        ProfileStatistic::Trimmed => {
            col.sort_by(f32::total_cmp);
            let keep = (n - n / 10).max(1);
            let t = 1.0 - keep as f64 / n as f64;
            let mean = col[..keep].iter().map(|v| *v as f64).sum::<f64>() / keep as f64;
            // Bias-Korrektur: Für exponentialverteilte Leistung ist der Mittelwert
            // der unteren (1−t) Anteile (1 − t·(1 − ln t)) / (1 − t) des wahren
            // Mittelwerts (t = 0,1 ⇒ 0,744, also −1,3 dB ohne Korrektur).
            let factor = if t > 0.0 {
                (1.0 - t * (1.0 - t.ln())) / (1.0 - t)
            } else {
                1.0
            };
            (mean / factor) as f32
        }
        ProfileStatistic::Median => {
            col.sort_by(f32::total_cmp);
            // Median einer Exponentialverteilung = ln 2 · Mittelwert.
            col[n / 2] / std::f32::consts::LN_2
        }
    }
}

// ------------------------------------------------------------ Konfidenz

/// Standardabweichung von 10·log10 einer exponentialverteilten Größe (χ², 2 FG).
pub const STATIONARY_LOG_STD_DB: f64 = 5.57;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Confidence {
    pub total: f64,
    pub amount: f64,
    pub separation: f64,
    pub stationarity: f64,
    pub spread: f64,
}

pub fn confidence(
    duration_s: f64,
    target_s: f64,
    separation_db: Option<f64>,
    stationarity_std_db: f64,
    pauses: usize,
) -> Confidence {
    let amount = (duration_s / target_s).clamp(0.0, 1.0);
    // Ohne Sprache (reiner Raumton) ist die Trennung kein Kriterium.
    let separation = separation_db.map_or(1.0, |s| ((s - 6.0) / 14.0).clamp(0.0, 1.0));
    let stationarity =
        (1.0 - (stationarity_std_db - STATIONARY_LOG_STD_DB).max(0.0) / 3.0).clamp(0.0, 1.0);
    let spread: f64 = match pauses {
        0 => 0.0,
        1 => 0.6,
        2 => 0.8,
        _ => 1.0,
    };
    let total =
        amount.powf(0.3) * separation.powf(0.3) * stationarity.powf(0.25) * spread.powf(0.15);
    Confidence {
        total,
        amount,
        separation,
        stationarity,
        spread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_power_conversion_roundtrip() {
        let p = vec![vec![1.0f32, 2.0, 4.0, 8.0, 16.0]];
        let prof =
            NoiseProfile::from_power(&p, 48_000, 8, ProfileSource::Auto, ProfileStatistic::Mean);
        let back = prof.power_for(48_000, 8, 1);
        for (a, b) in p[0].iter().zip(&back[0]) {
            assert!((a / b - 1.0).abs() < 1e-4);
        }
        let stereo = prof.power_for(48_000, 8, 2);
        assert_eq!(stereo.len(), 2);
        // Doppelte FFT-Größe: doppelte Rohleistung (+3 dB) bei gleicher PSD.
        let big = prof.power_for(48_000, 16, 1);
        assert!((big[0][2] / p[0][1] - 2.0).abs() < 1e-3);
    }

    #[test]
    fn statistics_are_unbiased_for_exponential_power() {
        // Exponentialverteilte Werte mit Mittelwert 1 (wie |X|² von Rauschen).
        let mut s = 12345u64;
        let mut vals: Vec<f32> = (0..20_000)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (-(((s >> 11) as f64 + 0.5) / (1u64 << 53) as f64).ln()) as f32
            })
            .collect();
        for stat in [
            ProfileStatistic::Mean,
            ProfileStatistic::Trimmed,
            ProfileStatistic::Median,
        ] {
            let v = aggregate(&mut vals, stat) as f64;
            assert!((10.0 * v.log10()).abs() < 0.1, "{stat:?}: {v}");
        }
    }

    #[test]
    fn confidence_ranges() {
        let good = confidence(3.0, 2.0, Some(30.0), 5.6, 5);
        assert!(good.total > 0.95);
        let short = confidence(0.3, 2.0, Some(30.0), 5.6, 1);
        assert!(short.total < 0.6);
        let unstable = confidence(3.0, 2.0, Some(30.0), 9.0, 5);
        assert!(unstable.total < 0.8);
        assert!(confidence(3.0, 2.0, Some(4.0), 5.6, 5).total == 0.0);
    }
}
