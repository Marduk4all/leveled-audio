//! Synthetisches, sprachähnliches Signal mit bekannter Aktivität.
//!
//! Quelle-Filter-Modell: Glottis-Pulsfolge (F0 mit Satzmelodie, Deklination
//! und Jitter) → spektrale Neigung → drei Formant-Resonatoren je Vokal.
//! Dazu Frikative (bandpassgefiltertes Rauschen), Wortlücken und Phrasenpausen.
//! Kein echter Sprachersatz, aber mit den Eigenschaften, auf die VAD und
//! Rauschreduktion reagieren: harmonische Struktur, Formanten, Onsets, Pausen.

use crate::Rng;
use std::f64::consts::PI;

pub struct SpeechSignal {
    pub samples: Vec<f32>,
    /// Aktive Bereiche (Silben und Frikative) als Sample-Bereiche `[start, end)`.
    pub active: Vec<(usize, usize)>,
}

impl SpeechSignal {
    /// Anteil aktiver Samples.
    pub fn activity(&self) -> f64 {
        let a: usize = self.active.iter().map(|(s, e)| e - s).sum();
        a as f64 / self.samples.len().max(1) as f64
    }

    pub fn is_active(&self, sample: usize) -> bool {
        self.active.iter().any(|&(s, e)| sample >= s && sample < e)
    }
}

const VOWELS: [(f64, f64, f64); 5] = [
    (730.0, 1090.0, 2440.0),
    (530.0, 1840.0, 2480.0),
    (270.0, 2290.0, 3010.0),
    (570.0, 840.0, 2410.0),
    (300.0, 870.0, 2240.0),
];

struct Resonator {
    a1: f64,
    a2: f64,
    g: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn new(freq: f64, bw: f64, rate: f64) -> Self {
        let r = (-PI * bw / rate).exp();
        let theta = 2.0 * PI * freq / rate;
        Resonator {
            a1: 2.0 * r * theta.cos(),
            a2: -r * r,
            g: 1.0 - r,
            y1: 0.0,
            y2: 0.0,
        }
    }
    fn set(&mut self, freq: f64, bw: f64, rate: f64) {
        let n = Resonator::new(freq, bw, rate);
        self.a1 = n.a1;
        self.a2 = n.a2;
        self.g = n.g;
    }
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Erzeugt `secs` Sekunden Sprache mit Grundfrequenz um `f0` Hz; der
/// RMS-Pegel der **aktiven** Abschnitte wird auf `level_dbfs` normiert.
pub fn synth_speech(rate: u32, secs: f64, f0: f64, level_dbfs: f64, seed: u64) -> SpeechSignal {
    let fs = rate as f64;
    let total = (secs * fs) as usize;
    let mut rng = Rng::new(seed);
    let mut out = vec![0f64; total];
    let mut active = Vec::new();
    let ms = |m: f64| (m / 1000.0 * fs) as usize;
    let mut f = [
        Resonator::new(500.0, 80.0, fs),
        Resonator::new(1500.0, 100.0, fs),
        Resonator::new(2500.0, 120.0, fs),
    ];
    let mut fric = Resonator::new(5000.0, 2500.0, fs);
    let mut tilt = 0.0f64;
    let mut phase = 0.0f64;

    // Kleine Anfangspause.
    let mut t = ms(300.0 + 200.0 * (rng.uniform() + 1.0));
    while t < total {
        // Phrase
        let phrase_len = ms(1500.0 + 1250.0 * (rng.uniform() + 1.0));
        let phrase_end = (t + phrase_len).min(total);
        let phrase_start = t;
        let mut syl_in_word = 0;
        while t < phrase_end {
            // Optionaler Frikativ vor der Silbe.
            if rng.uniform() > 0.4 {
                let len = ms(60.0 + 30.0 * (rng.uniform() + 1.0)).min(total - t);
                for i in 0..len {
                    let env = (PI * i as f64 / len as f64).sin();
                    out[t + i] += 0.25 * env * fric.tick(rng.uniform());
                }
                active.push((t, t + len));
                t += len;
            }
            if t >= total {
                break;
            }
            // Vokal-Silbe
            let v = VOWELS[(rng.next_u64() % 5) as usize];
            for (r, (fq, bw)) in f.iter_mut().zip([(v.0, 80.0), (v.1, 100.0), (v.2, 120.0)]) {
                r.set(fq, bw, fs);
            }
            let len = ms(120.0 + 90.0 * (rng.uniform() + 1.0)).min(total - t);
            let att = ms(15.0).max(1);
            let rel = ms(35.0).max(1);
            let amp = 0.7 + 0.3 * rng.uniform();
            for i in 0..len {
                let pos = (t + i - phrase_start) as f64 / phrase_len as f64;
                // Satzmelodie: Anstieg, dann Deklination; 1 % Jitter.
                let f0_now =
                    f0 * (1.0 + 0.12 * (PI * pos).sin() - 0.1 * pos) * (1.0 + 0.01 * rng.uniform());
                phase += f0_now / fs;
                let pulse = if phase >= 1.0 {
                    phase -= 1.0;
                    1.0
                } else {
                    0.0
                };
                // Glottale Neigung durch einen Tiefpass-Pol; zusammen mit den
                // Formanten ergibt sich eine plausible spektrale Hüllkurve.
                tilt = 0.96 * tilt + pulse;
                let x = tilt;
                let y = f[0].tick(x) * 1.0 + f[1].tick(x) * 0.6 + f[2].tick(x) * 0.3;
                let env = if i < att {
                    i as f64 / att as f64
                } else if i + rel > len {
                    (len - i) as f64 / rel as f64
                } else {
                    1.0
                };
                out[t + i] += amp * env * y;
            }
            active.push((t, t + len));
            t += len;
            syl_in_word += 1;
            // Wortlücke nach 1–3 Silben.
            if syl_in_word > (rng.next_u64() % 3) as usize {
                t += ms(40.0 + 40.0 * (rng.uniform() + 1.0));
                syl_in_word = 0;
            }
        }
        // Phrasenpause
        t += ms(400.0 + 400.0 * (rng.uniform() + 1.0));
    }

    // Aktive Bereiche zusammenfassen und Pegel normieren.
    active.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in active {
        match merged.last_mut() {
            Some(m) if s <= m.1 => m.1 = m.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let (mut sum, mut cnt) = (0f64, 0usize);
    for &(s, e) in &merged {
        for v in &out[s..e] {
            sum += v * v;
        }
        cnt += e - s;
    }
    let rms = (sum / cnt.max(1) as f64).sqrt().max(1e-30);
    let g = crate::db_to_amp(level_dbfs) / rms;
    SpeechSignal {
        samples: out.iter().map(|v| (v * g) as f32).collect(),
        active: merged,
    }
}

/// Kurtosis E[x²]/E[x]² der übergebenen (nichtnegativen) Werte, z. B. Bin-Leistungen.
/// Die Log-Kurtosis-Ratio ln(kurt_nachher / kurt_vorher) des Restrauschens ist
/// ein gängiges Maß für Musical Noise (≈ 0: natürlich, groß: „zwitschernd“).
pub fn power_kurtosis(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    let m1 = values.iter().sum::<f64>() / n;
    let m2 = values.iter().map(|v| v * v).sum::<f64>() / n;
    m2 / (m1 * m1).max(1e-300)
}
