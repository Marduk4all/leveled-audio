//! Testsignale und Hilfsfunktionen. Nur für Tests und Benchmarks.
//!
//! Alle Generatoren sind deterministisch (fester PRNG-Seed) und liefern planare
//! Kanalpuffer (`Vec<Vec<f32>>`).

mod speech;

use leveled_audio_io::{AudioSpec, SampleFormat, create_sink};
pub use speech::{SpeechSignal, power_kurtosis, synth_speech};
use std::f64::consts::PI;
use std::path::Path;

pub type Planar = Vec<Vec<f32>>;

pub fn db_to_amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Sinus mit Spitzenamplitude `peak_dbfs`, identisch auf allen Kanälen.
pub fn sine(rate: u32, channels: usize, freq: f64, peak_dbfs: f64, secs: f64) -> Planar {
    sine_phase(rate, channels, freq, peak_dbfs, secs, 0.0)
}

pub fn sine_phase(
    rate: u32,
    channels: usize,
    freq: f64,
    peak_dbfs: f64,
    secs: f64,
    phase: f64,
) -> Planar {
    let n = (secs * rate as f64).round() as usize;
    let a = db_to_amp(peak_dbfs);
    let ch: Vec<f32> = (0..n)
        .map(|i| (a * (2.0 * PI * freq * i as f64 / rate as f64 + phase).sin()) as f32)
        .collect();
    vec![ch; channels]
}

/// Folge von 1-kHz-Sinusabschnitten `(peak_dbfs, sekunden)` mit
/// durchlaufender Phase (wie in EBU Tech 3341/3342).
pub fn tone_sequence(rate: u32, channels: usize, freq: f64, segments: &[(f64, f64)]) -> Planar {
    let mut out = Vec::new();
    let mut i = 0usize;
    for &(db, secs) in segments {
        let a = db_to_amp(db);
        let n = (secs * rate as f64).round() as usize;
        for _ in 0..n {
            out.push((a * (2.0 * PI * freq * i as f64 / rate as f64).sin()) as f32);
            i += 1;
        }
    }
    vec![out; channels]
}

/// Einfacher, plattformunabhängiger PRNG (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Gleichverteilt in [-1, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// Weißes Rauschen mit gegebenem RMS-Pegel (dBFS), unabhängig pro Kanal.
pub fn white_noise(rate: u32, channels: usize, rms_dbfs: f64, secs: f64, seed: u64) -> Planar {
    let n = (secs * rate as f64).round() as usize;
    // Gleichverteilung in [-1,1) hat RMS 1/sqrt(3).
    let g = db_to_amp(rms_dbfs) * 3f64.sqrt();
    (0..channels)
        .map(|c| {
            let mut rng = Rng::new(seed.wrapping_add(c as u64 * 7919));
            (0..n).map(|_| (rng.uniform() * g) as f32).collect()
        })
        .collect()
}

/// Rosa Rauschen (Filter nach Paul Kellet), auf `rms_dbfs` normiert.
pub fn pink_noise(rate: u32, channels: usize, rms_dbfs: f64, secs: f64, seed: u64) -> Planar {
    let white = white_noise(rate, channels, 0.0, secs, seed);
    white
        .into_iter()
        .map(|w| {
            let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) =
                (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            let mut out: Vec<f64> = w
                .iter()
                .map(|&x| {
                    let x = x as f64;
                    b0 = 0.99886 * b0 + x * 0.0555179;
                    b1 = 0.99332 * b1 + x * 0.0750759;
                    b2 = 0.96900 * b2 + x * 0.1538520;
                    b3 = 0.86650 * b3 + x * 0.3104856;
                    b4 = 0.55000 * b4 + x * 0.5329522;
                    b5 = -0.7616 * b5 - x * 0.0168980;
                    let y = b0 + b1 + b2 + b3 + b4 + b5 + b6 + x * 0.5362;
                    b6 = x * 0.115926;
                    y
                })
                .collect();
            let rms = (out.iter().map(|v| v * v).sum::<f64>() / out.len().max(1) as f64).sqrt();
            let g = db_to_amp(rms_dbfs) / rms.max(1e-30);
            out.iter_mut().for_each(|v| *v *= g);
            out.into_iter().map(|v| v as f32).collect()
        })
        .collect()
}

/// Mischt `b` (mit Gain in dB) auf `a`. Länge = Minimum.
pub fn mix(a: &Planar, b: &Planar, gain_db: f64) -> Planar {
    let g = db_to_amp(gain_db) as f32;
    a.iter()
        .zip(b)
        .map(|(x, y)| x.iter().zip(y).map(|(p, q)| p + g * q).collect())
        .collect()
}

/// Hängt Signale zeitlich aneinander.
pub fn concat(parts: &[Planar]) -> Planar {
    let ch = parts[0].len();
    (0..ch)
        .map(|c| parts.iter().flat_map(|p| p[c].iter().copied()).collect())
        .collect()
}

pub fn silence(rate: u32, channels: usize, secs: f64) -> Planar {
    vec![vec![0.0; (secs * rate as f64).round() as usize]; channels]
}

/// "Sprachähnliches" Testsignal für Pegel- und Limitertests: bandbegrenztes
/// Rauschen mit 4-Hz-Silbenhüllkurve, Pausen und gelegentlichen Spitzen.
/// (Die realistischere Glottis-/Formant-Synthese folgt in Phase 2.)
pub fn speechlike(rate: u32, channels: usize, level_dbfs: f64, secs: f64, seed: u64) -> Planar {
    let n = (secs * rate as f64).round() as usize;
    let mut rng = Rng::new(seed);
    let mut lp = 0.0f64;
    let mut hp_prev = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / rate as f64;
        // Phrasen von 3 s, davon 2,4 s Sprache und 0,6 s Pause.
        let in_phrase = (t % 3.0) < 2.4;
        let syll = 0.5 - 0.5 * (2.0 * PI * 4.0 * t).cos();
        let env = if in_phrase { syll.powf(1.5) } else { 0.0 };
        let x = rng.uniform();
        lp += 0.15 * (x - lp); // grobe Tiefpass-Färbung
        let hp = lp - hp_prev; // und etwas Hochpass
        hp_prev = lp;
        // Gelegentliche Plosiv-Spitzen.
        let spike = if in_phrase && i % (rate as usize * 2) == rate as usize / 3 {
            8.0
        } else {
            1.0
        };
        out.push(env * (lp + 2.0 * hp) * spike);
    }
    let rms = (out.iter().map(|v| v * v).sum::<f64>() / n.max(1) as f64).sqrt();
    let g = db_to_amp(level_dbfs) / rms.max(1e-30);
    let ch: Vec<f32> = out.into_iter().map(|v| (v * g) as f32).collect();
    vec![ch; channels]
}

/// Schreibt planare Daten als WAV (für Integrationstests der CLI/Pipeline).
pub fn write_wav(path: &Path, rate: u32, format: SampleFormat, data: &Planar) {
    let spec = AudioSpec {
        sample_rate: rate,
        channels: data.len() as u16,
        format,
    };
    let mut sink = create_sink(path, spec).expect("WAV anlegen");
    let frames = data[0].len();
    let block = 65_536;
    let mut buf = vec![Vec::new(); data.len()];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        for (b, d) in buf.iter_mut().zip(data) {
            b.clear();
            b.extend_from_slice(&d[pos..pos + n]);
        }
        sink.write(&buf, n).expect("WAV schreiben");
        pos += n;
    }
    sink.finalize().expect("WAV abschließen");
}

/// Liest eine WAV komplett (nur für kurze Testdateien).
pub fn read_wav(path: &Path) -> (AudioSpec, Planar) {
    let mut src = leveled_audio_io::open_source(path).expect("WAV öffnen");
    let spec = src.spec();
    let mut all = vec![Vec::new(); spec.channels as usize];
    let mut buf = vec![Vec::new(); spec.channels as usize];
    loop {
        let n = src.read(&mut buf, 65_536).expect("WAV lesen");
        if n == 0 {
            break;
        }
        for (a, b) in all.iter_mut().zip(&buf) {
            a.extend_from_slice(b);
        }
    }
    (spec, all)
}

/// Hochgenauer True-Peak (16× Oversampling per Sinc-Interpolation), als
/// unabhängige Referenz zur Prüfung des Limiters. Langsam; nur für Tests.
pub fn reference_true_peak(data: &Planar) -> f64 {
    const OS: usize = 16;
    const HALF: isize = 64;
    let mut peak = 0f64;
    for ch in data {
        let n = ch.len() as isize;
        for i in 0..n {
            peak = peak.max(ch[i as usize].abs() as f64);
            for p in 1..OS {
                let t = i as f64 + p as f64 / OS as f64;
                let mut acc = 0.0;
                for k in (i - HALF + 1)..=(i + HALF) {
                    if k < 0 || k >= n {
                        continue;
                    }
                    let d = t - k as f64;
                    // Hann-gefensterte Sinc.
                    let w = 0.5 + 0.5 * (PI * d / (HALF as f64 + 1.0)).cos();
                    acc += ch[k as usize] as f64 * (PI * d).sin() / (PI * d) * w;
                }
                peak = peak.max(acc.abs());
            }
        }
    }
    peak
}

pub fn amp_to_db(a: f64) -> f64 {
    20.0 * a.max(1e-20).log10()
}

/// Testdateien unter `target/` ablegen, damit sie nicht im Repo landen.
pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("leveled-audio-tests")
        .join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Temp-Verzeichnis");
    dir
}

pub use leveled_audio_io::SampleFormat as Format;

/// Kurzform für Tests: Sample-Peak in dBFS.
pub fn sample_peak_db(data: &Planar) -> f64 {
    let p = data
        .iter()
        .flat_map(|c| c.iter())
        .fold(0f32, |m, x| m.max(x.abs()));
    amp_to_db(p as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_noise_level() {
        let n = white_noise(48_000, 1, -20.0, 2.0, 1);
        let rms =
            (n[0].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n[0].len() as f64).sqrt();
        assert!((amp_to_db(rms) + 20.0).abs() < 0.1);
    }

    #[test]
    fn reference_tp_finds_intersample_peak() {
        // fs/4-Sinus mit 45° Phase: Samples bei ±0,707·A, wahrer Peak A.
        let s = sine_phase(48_000, 1, 12_000.0, 0.0, 0.05, PI / 4.0);
        assert!((sample_peak_db(&s) + 3.01).abs() < 0.05);
        assert!(amp_to_db(reference_true_peak(&s)).abs() < 0.1);
    }
}

#[cfg(test)]
mod speech_tests {
    use super::*;

    #[test]
    fn synth_speech_level_and_activity() {
        let s = synth_speech(48_000, 20.0, 120.0, -20.0, 5);
        assert_eq!(s.samples.len(), 960_000);
        let a = s.activity();
        assert!((0.4..0.85).contains(&a), "Aktivität {a}");
        let (mut sum, mut cnt) = (0f64, 0usize);
        for &(b, e) in &s.active {
            for v in &s.samples[b..e] {
                sum += (*v as f64).powi(2);
            }
            cnt += e - b;
        }
        let db = amp_to_db((sum / cnt as f64).sqrt());
        assert!((db + 20.0).abs() < 0.01);
        // Pausen sind wirklich still.
        let silent = (0..s.samples.len())
            .filter(|&i| !s.is_active(i))
            .step_by(997)
            .all(|i| s.samples[i] == 0.0);
        assert!(silent);
    }
}
