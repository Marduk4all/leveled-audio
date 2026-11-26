//! Adaptiver Leveler: Gain-Kurve offline aus den Pass-1-Daten (PLAN 3.6).
//!
//! 1. Sprechenergie je Frame: K-gewichtete Leistung minus K-gewichtete
//!    Rauschleistung (Untergrenze −70 dBFS).
//! 2. Phrasen = zusammenhängende VAD-Sprachbereiche. Pegel einer Phrase:
//!    Energiemittel; Phrasen über 6 s werden mit gleitendem 3-s-Fenster gemessen.
//! 3. Referenz = Mitte zwischen unterem und oberem Quartil der Phrasenpegel
//!    (dauergewichtet). Bei einem Sprecher ≈ Median; bei zwei Sprechern liegt
//!    sie zwischen beiden, sodass leise angehoben und laute abgesenkt werden
//!    (der Median fiele zufällig in einen der beiden Cluster).
//!    Phrasen mehr als `gate_below_ref_db` unter der Referenz (Atmer, Raumgeräusch,
//!    weit entferntes Gemurmel) werden wie Pausen behandelt, also nicht angehoben.
//! 4. Soll-Gain = Stärke · (Referenz − Pegel), begrenzt auf ±Grenzen.
//!    Phrasen unter 300 ms („mhm“) übernehmen den Gain der Nachbarphrase.
//! 5. Innerhalb einer Phrase: Slew-Limit und nullphasige Glättung (kein Pumpen).
//! 6. Wechsel zwischen Phrasen liegen in den Pausen (Cosinus-Rampe, die vor dem
//!    Sprachbeginn endet). Lange Pausen werden nicht über `pause_max_boost_db`
//!    angehoben, kurze halten den Gain (sonst „atmet“ der Rauschteppich).

use crate::features::FrameFeatures;
use crate::vad::FrameClass;
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub struct LevelerParams {
    /// 0–1; 0 ⇒ Kurve ist überall 0 dB.
    pub strength: f64,
    pub max_boost_db: f64,
    pub max_cut_db: f64,
    /// Obergrenze des Gains in langen Pausen.
    pub pause_max_boost_db: f64,
    /// Maximale Gain-Änderung innerhalb einer Phrase.
    pub slew_db_per_s: f64,
    /// Phrasen, die so weit unter der Referenz liegen, gelten als Pause.
    pub gate_below_ref_db: f64,
    pub min_segment_s: f64,
    pub long_segment_s: f64,
    pub window_s: f64,
    pub long_pause_s: f64,
    pub pause_ramp_s: f64,
    pub long_pause_ramp_s: f64,
    pub smooth_tau_s: f64,
    pub floor_dbfs: f64,
}

impl Default for LevelerParams {
    fn default() -> Self {
        LevelerParams {
            strength: 0.6,
            max_boost_db: 10.0,
            max_cut_db: 10.0,
            pause_max_boost_db: 0.0,
            slew_db_per_s: 2.0,
            gate_below_ref_db: 20.0,
            min_segment_s: 0.3,
            long_segment_s: 6.0,
            window_s: 3.0,
            long_pause_s: 2.0,
            pause_ramp_s: 0.25,
            long_pause_ramp_s: 0.5,
            smooth_tau_s: 0.15,
            floor_dbfs: -70.0,
        }
    }
}

/// Gain-Kurve mit einer Stützstelle je STFT-Frame (Frame-Mitte).
#[derive(Debug, Clone)]
pub struct LevelerCurve {
    pub gain_db: Vec<f32>,
    pub hop: usize,
    /// Sample-Position der Stützstelle von Frame 0 (kann negativ sein).
    pub first_center: i64,
    pub stats: LevelerStats,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LevelerStats {
    /// Anzahl gelevelter Phrasen.
    pub phrases: usize,
    /// Als Sprache erkannt, aber zu leise (wie Pause behandelt).
    pub gated: usize,
    /// Referenzpegel (K-gewichtet, rauschkompensiert, dBFS).
    pub reference_db: Option<f64>,
    /// Gain-Bereich in Sprache.
    pub gain_min_db: f64,
    pub gain_max_db: f64,
    /// Mittlerer |Gain| über Sprach-Frames.
    pub mean_abs_gain_db: f64,
}

impl LevelerCurve {
    /// Gain (dB) an einer Sample-Position, linear zwischen den Stützstellen.
    pub fn gain_db_at(&self, sample: i64) -> f32 {
        let n = self.gain_db.len();
        if n == 0 {
            return 0.0;
        }
        let u = sample - self.first_center;
        if u <= 0 {
            return self.gain_db[0];
        }
        let k = (u / self.hop as i64) as usize;
        if k + 1 >= n {
            return self.gain_db[n - 1];
        }
        let frac = (u % self.hop as i64) as f32 / self.hop as f32;
        self.gain_db[k] + frac * (self.gain_db[k + 1] - self.gain_db[k])
    }
}

fn segments(classes: &[FrameClass]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (k, c) in classes.iter().enumerate() {
        match (start, *c == FrameClass::Speech) {
            (None, true) => start = Some(k),
            (Some(a), false) => {
                out.push((a, k));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(a) = start {
        out.push((a, classes.len()));
    }
    out
}

/// Mitte zwischen 25- und 75-%-Quantil.
fn quartile_mid(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    Some(0.5 * (q(0.25) + q(0.75)))
}

/// Zweiseitiges Slew-Limit: |x[k] − x[k−1]| ≤ step.
fn slew_limit(x: &mut [f64], step: f64) {
    for k in 1..x.len() {
        x[k] = x[k].clamp(x[k - 1] - step, x[k - 1] + step);
    }
    for k in (0..x.len().saturating_sub(1)).rev() {
        x[k] = x[k].clamp(x[k + 1] - step, x[k + 1] + step);
    }
}

/// Nullphasige Ein-Pol-Glättung (vorwärts, dann rückwärts).
fn smooth(x: &mut [f64], alpha: f64) {
    if x.is_empty() {
        return;
    }
    let mut y = x[0];
    for v in x.iter_mut() {
        y = alpha * y + (1.0 - alpha) * *v;
        *v = y;
    }
    let mut y = x[x.len() - 1];
    for v in x.iter_mut().rev() {
        y = alpha * y + (1.0 - alpha) * *v;
        *v = y;
    }
}

/// Cosinus-Rampe von `from` nach `to` über `out` (letzter Wert = `to`).
fn ramp(out: &mut [f64], from: f64, to: f64) {
    let r = out.len();
    for (i, v) in out.iter_mut().enumerate() {
        let t = (i + 1) as f64 / r as f64;
        let w = 0.5 - 0.5 * (std::f64::consts::PI * t).cos();
        *v = from + (to - from) * w;
    }
}

/// Berechnet die Leveler-Kurve. `noise_k_power`: K-gewichtete Rauschleistung
/// (linear, wie [`crate::k_weighted_power`]), 0 wenn unbekannt.
pub fn leveler_curve(
    f: &FrameFeatures,
    classes: &[FrameClass],
    noise_k_power: f64,
    p: &LevelerParams,
) -> LevelerCurve {
    let n = f.len().min(classes.len());
    let hop_s = f.hop_seconds();
    let mut curve = LevelerCurve {
        gain_db: vec![0.0; n],
        hop: f.hop,
        first_center: f.hop as i64 - f.fft_size as i64 / 2,
        stats: LevelerStats::default(),
    };
    let mut segs = segments(&classes[..n]);
    curve.stats.phrases = segs.len();
    if segs.is_empty() {
        return curve;
    }

    // 1. Sprechleistung je Frame.
    let floor = 10f64.powf(p.floor_dbfs / 10.0);
    let power: Vec<f64> = f.k_db[..n]
        .iter()
        .map(|&db| (10f64.powf(db as f64 / 10.0) - noise_k_power).max(floor))
        .collect();
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0f64);
    for v in &power {
        prefix.push(prefix.last().unwrap() + v);
    }
    let mean_db = |a: usize, b: usize| 10.0 * ((prefix[b] - prefix[a]) / (b - a) as f64).log10();

    // 2. Pegel je Sprach-Frame.
    let mut level = vec![0f64; n];
    let half = f.frames_for(p.window_s / 2.0);
    for &(a, b) in &segs {
        if (b - a) as f64 * hop_s > p.long_segment_s {
            for (k, l) in level.iter_mut().enumerate().take(b).skip(a) {
                *l = mean_db(k.saturating_sub(half).max(a), (k + half + 1).min(b));
            }
        } else {
            let m = mean_db(a, b);
            level[a..b].iter_mut().for_each(|l| *l = m);
        }
    }
    let is_short = |&(a, b): &(usize, usize)| ((b - a) as f64 * hop_s) < p.min_segment_s;

    // 3. Referenz über die Frames der (nicht zu kurzen) Phrasen.
    let pool = |skip_short: bool| -> Vec<f64> {
        segs.iter()
            .filter(|s| !(skip_short && is_short(s)))
            .flat_map(|&(a, b)| level[a..b].iter().copied())
            .collect()
    };
    let mut values = pool(true);
    if values.is_empty() {
        values = pool(false);
    }
    let reference = quartile_mid(values).expect("mindestens eine Phrase");
    curve.stats.reference_db = Some(reference);

    // Sehr leise „Phrasen“ nicht leveln, sondern wie Pausen behandeln.
    let before = segs.len();
    segs.retain(|&(a, b)| mean_db(a, b) >= reference - p.gate_below_ref_db);
    curve.stats.gated = before - segs.len();
    curve.stats.phrases = segs.len();
    // Ohne Stärke bleibt die Kurve flach; die Referenz dient dann nur dem Kompressor.
    if segs.is_empty() || p.strength <= 0.0 {
        return curve;
    }

    // 4./5. Soll-Gain je Phrase, Slew-Limit und Glättung innerhalb der Phrase.
    let mut g = vec![0f64; n];
    let step = p.slew_db_per_s * hop_s;
    let alpha = (-hop_s / p.smooth_tau_s).exp();
    for &(a, b) in segs.iter().filter(|s| !is_short(s)) {
        for k in a..b {
            g[k] = (p.strength * (reference - level[k])).clamp(-p.max_cut_db, p.max_boost_db);
        }
        slew_limit(&mut g[a..b], step);
        smooth(&mut g[a..b], alpha);
    }
    // Kurze Phrasen: Gain der zeitlich nächsten langen Phrase.
    let long: Vec<usize> = (0..segs.len()).filter(|&i| !is_short(&segs[i])).collect();
    for (i, &(a, b)) in segs.iter().enumerate() {
        if !is_short(&(a, b)) {
            continue;
        }
        let prev = long.iter().rev().find(|&&j| j < i).map(|&j| segs[j]);
        let next = long.iter().find(|&&j| j > i).map(|&j| segs[j]);
        let v = match (prev, next) {
            (Some(pv), Some(nx)) if a - pv.1 <= nx.0 - b => g[pv.1 - 1],
            (_, Some(nx)) => g[nx.0],
            (Some(pv), None) => g[pv.1 - 1],
            (None, None) => {
                (p.strength * (reference - level[a])).clamp(-p.max_cut_db, p.max_boost_db)
            }
        };
        g[a..b].iter_mut().for_each(|x| *x = v);
    }

    // 6. Pausen.
    let ramp_f = f.frames_for(p.pause_ramp_s);
    let long_ramp_f = f.frames_for(p.long_pause_ramp_s);
    let is_long = |len: usize| len as f64 * hop_s >= p.long_pause_s;
    // Pausenobergrenze, aber nie unter die Absenkgrenze.
    let pmb = p.pause_max_boost_db.max(-p.max_cut_db);
    // Vor der ersten Phrase: Es gibt keinen vorherigen Gain zu halten, also
    // wie eine lange Pause behandeln (nicht über `pause_max_boost_db` anheben).
    // Sonst würden Geräusche am Dateianfang mit dem Gain der ersten Phrase
    // angehoben.
    let (s0, _) = segs[0];
    let gn = g[s0];
    let q = gn.min(pmb);
    let r = long_ramp_f.min(s0);
    g[..s0 - r].iter_mut().for_each(|x| *x = q);
    ramp(&mut g[s0 - r..s0], q, gn);
    // Zwischen den Phrasen.
    for w in segs.windows(2) {
        let (e, s) = (w[0].1, w[1].0);
        let (gp, gn) = (g[e - 1], g[s]);
        let len = s - e;
        if is_long(len) {
            let q = gp.min(pmb);
            ramp(&mut g[e..e + long_ramp_f], gp, q);
            g[e + long_ramp_f..s - long_ramp_f]
                .iter_mut()
                .for_each(|x| *x = q);
            ramp(&mut g[s - long_ramp_f..s], q, gn);
        } else {
            let r = ramp_f.min(len);
            g[e..s - r].iter_mut().for_each(|x| *x = gp);
            ramp(&mut g[s - r..s], gp, gn);
        }
    }
    // Nach der letzten Phrase.
    let e = segs[segs.len() - 1].1;
    // Nach der letzten Phrase ebenso (nichts folgt, das „atmen“ könnte).
    if e < n {
        let gp = g[e - 1];
        let q = gp.min(pmb);
        let r = long_ramp_f.min(n - e);
        ramp(&mut g[e..e + r], gp, q);
        g[e + r..].iter_mut().for_each(|x| *x = q);
    }

    // Statistik über Sprach-Frames.
    let speech: Vec<f64> = segs.iter().flat_map(|&(a, b)| g[a..b].to_vec()).collect();
    curve.stats.gain_min_db = speech.iter().copied().fold(f64::INFINITY, f64::min);
    curve.stats.gain_max_db = speech.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    curve.stats.mean_abs_gain_db =
        speech.iter().map(|v| v.abs()).sum::<f64>() / speech.len() as f64;
    curve.gain_db = g.iter().map(|v| *v as f32).collect();
    curve
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOP: usize = 512;
    const RATE: u32 = 48_000;

    fn hop_s() -> f64 {
        HOP as f64 / RATE as f64
    }

    /// Features und Klassen aus einer Liste (Dauer s, Pegel dB oder None = Pause).
    fn scene(parts: &[(f64, Option<f32>)]) -> (FrameFeatures, Vec<FrameClass>) {
        let mut f = FrameFeatures {
            sample_rate: RATE,
            fft_size: 4 * HOP,
            hop: HOP,
            ..Default::default()
        };
        let mut classes = Vec::new();
        for &(secs, lvl) in parts {
            let frames = (secs / hop_s()).round() as usize;
            for _ in 0..frames {
                f.k_db.push(lvl.unwrap_or(-60.0));
                classes.push(if lvl.is_some() {
                    FrameClass::Speech
                } else {
                    FrameClass::Pause
                });
            }
        }
        f.e_db = f.k_db.clone();
        (f, classes)
    }

    fn rng(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    #[test]
    fn equalizes_two_speakers_by_strength() {
        let mut parts = vec![(1.0, None)];
        for _ in 0..6 {
            parts.push((4.0, Some(-20.0)));
            parts.push((0.8, None));
            parts.push((4.0, Some(-30.0)));
            parts.push((0.8, None));
        }
        let (f, c) = scene(&parts);
        for (strength, expect) in [(1.0, 0.0), (0.5, 5.0), (0.0, 10.0)] {
            let p = LevelerParams {
                strength,
                ..Default::default()
            };
            let curve = leveler_curve(&f, &c, 0.0, &p);
            // Mitte der ersten beiden Phrasen.
            let mid = |t: f64| curve.gain_db[(t / hop_s()) as usize] as f64;
            let diff = (-20.0 + mid(3.0)) - (-30.0 + mid(7.8));
            assert!((diff - expect).abs() < 0.2, "{strength}: {diff:.2}");
            // Symmetrisch: laut wird so weit abgesenkt, wie leise angehoben wird.
            assert!(
                (mid(3.0) + mid(7.8)).abs() < 0.2,
                "{} {}",
                mid(3.0),
                mid(7.8)
            );
        }
    }

    #[test]
    fn noise_compensation_raises_estimate_accuracy() {
        // Leise Phrase knapp über dem Rauschen: ohne Kompensation zu laut geschätzt.
        let (f, c) = scene(&[
            (1.0, None),
            (4.0, Some(-20.0)),
            (1.0, None),
            (4.0, Some(-37.0)),
            (1.0, None),
        ]);
        let noise = 10f64.powf(-40.0 / 10.0);
        let p = LevelerParams {
            strength: 1.0,
            max_boost_db: 20.0,
            max_cut_db: 20.0,
            ..Default::default()
        };
        let comp = leveler_curve(&f, &c, noise, &p);
        let raw = leveler_curve(&f, &c, 0.0, &p);
        let at = |cv: &LevelerCurve, t: f64| cv.gain_db[(t / hop_s()) as usize] as f64;
        // Wahre Sprechpegel: −20 und 10·log10(10^−3,7 − 10^−4) ≈ −40,0 … also Differenz ≈ 20 dB.
        let d_comp = at(&comp, 8.0) - at(&comp, 3.0);
        let d_raw = at(&raw, 8.0) - at(&raw, 3.0);
        assert!(d_comp > d_raw + 2.0, "{d_comp:.2} vs {d_raw:.2}");
    }

    #[test]
    fn property_bounds_slew_and_pauses() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        for case in 0..200 {
            let mut parts = Vec::new();
            let count = 2 + (rng(&mut seed) * 12.0) as usize;
            for _ in 0..count {
                let speech = (
                    rng(&mut seed) * 9.0 + 0.1,
                    Some((-45.0 + rng(&mut seed) * 40.0) as f32),
                );
                let pause = (rng(&mut seed) * 4.0 + 0.25, None);
                parts.push(pause);
                parts.push(speech);
            }
            parts.push((rng(&mut seed) * 3.0, None));
            let (mut f, c) = scene(&parts);
            // Pegelschwankungen innerhalb der Phrasen.
            for v in f.k_db.iter_mut() {
                *v += (rng(&mut seed) * 12.0 - 6.0) as f32;
            }
            let p = LevelerParams {
                strength: rng(&mut seed),
                max_boost_db: rng(&mut seed) * 20.0,
                max_cut_db: rng(&mut seed) * 20.0,
                pause_max_boost_db: rng(&mut seed) * 20.0 - 10.0,
                slew_db_per_s: 0.5 + rng(&mut seed) * 9.5,
                ..Default::default()
            };
            let cv = leveler_curve(&f, &c, 0.0, &p);
            let eps = 1e-4;
            for &g in &cv.gain_db {
                let g = g as f64;
                assert!(
                    g <= p.max_boost_db + eps && g >= -p.max_cut_db - eps,
                    "Fall {case}: {g}"
                );
            }
            let segs = segments(&c);
            let step = p.slew_db_per_s * hop_s();
            for &(a, b) in &segs {
                for k in a + 1..b {
                    let d = (cv.gain_db[k] - cv.gain_db[k - 1]).abs() as f64;
                    assert!(d <= step + eps, "Fall {case}: Slew {d} > {step}");
                }
            }
            // Inneres langer Pausen (≥ 2 s): Gain ≤ pause_max_boost.
            let ramp = (0.5 / hop_s()).round() as usize;
            for w in segs.windows(2) {
                let (e, s) = (w[0].1, w[1].0);
                if (s - e) as f64 * hop_s() >= 2.0 {
                    for k in e + ramp..s - ramp {
                        assert!(
                            cv.gain_db[k] as f64 <= p.pause_max_boost_db.max(-p.max_cut_db) + eps,
                            "Fall {case}: Pause {k}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn constant_level_long_phrase_does_not_pump() {
        // 20 s Monolog mit realistischer Pegelschwankung je Frame (±6 dB).
        let (mut f, c) = scene(&[(1.0, None), (20.0, Some(-30.0)), (1.0, None)]);
        let mut seed = 99u64;
        for v in f.k_db.iter_mut().skip(100) {
            *v += (rng(&mut seed) * 12.0 - 6.0) as f32;
        }
        let p = LevelerParams {
            strength: 1.0,
            ..Default::default()
        };
        // Referenz = eigener Pegel; um eine Abweichung zu erzwingen, eine zweite
        // leisere Phrase dazunehmen wäre unnötig: die Schwankung allein zählt.
        let cv = leveler_curve(&f, &c, 0.0, &p);
        let seg = &cv.gain_db[(1.0 / hop_s()) as usize..(21.0 / hop_s()) as usize];
        let (lo, hi) = seg
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(hi - lo < 1.0, "Schwankung {:.2} dB", hi - lo);
    }

    #[test]
    fn zero_strength_and_no_speech_give_flat_curve() {
        let (f, c) = scene(&[(1.0, None), (4.0, Some(-20.0)), (1.0, None)]);
        let p = LevelerParams {
            strength: 0.0,
            ..Default::default()
        };
        assert!(
            leveler_curve(&f, &c, 0.0, &p)
                .gain_db
                .iter()
                .all(|g| *g == 0.0)
        );
        let (f, c) = scene(&[(3.0, None)]);
        let cv = leveler_curve(&f, &c, 0.0, &LevelerParams::default());
        assert!(cv.gain_db.iter().all(|g| *g == 0.0));
        assert_eq!(cv.stats.phrases, 0);
    }

    #[test]
    fn short_interjection_takes_neighbour_gain() {
        let (f, c) = scene(&[
            (1.0, None),
            (4.0, Some(-25.0)),
            (0.5, None),
            (0.2, Some(-10.0)),
            (0.5, None),
            (4.0, Some(-25.0)),
            (1.0, None),
        ]);
        let cv = leveler_curve(&f, &c, 0.0, &LevelerParams::default());
        let at = |t: f64| cv.gain_db[(t / hop_s()) as usize];
        assert!(
            (at(5.6) - at(3.0)).abs() < 0.1,
            "{} vs {}",
            at(5.6),
            at(3.0)
        );
    }

    #[test]
    fn very_quiet_phrase_is_not_boosted() {
        // Atmer o. Ä. 25 dB unter der Sprache am Dateiende: kein +10 dB.
        let (f, c) = scene(&[
            (1.0, None),
            (4.0, Some(-20.0)),
            (0.5, None),
            (4.0, Some(-24.0)),
            (0.5, None),
            (0.8, Some(-47.0)),
            (0.5, None),
        ]);
        let cv = leveler_curve(&f, &c, 0.0, &LevelerParams::default());
        let at = |t: f64| cv.gain_db[(t / hop_s()) as usize];
        assert_eq!(cv.stats.gated, 1);
        assert_eq!(cv.stats.phrases, 2);
        // Nach der letzten echten Phrase (angehoben um ≈ +1,2 dB) gleitet der
        // Gain auf höchstens `pause_max_boost_db` (0): Der leise Ausklang wird
        // nicht mit angehoben (seit Phase 5 auch bei kurzen End-Bereichen).
        assert!(at(9.4) > 0.5, "{}", at(9.4));
        assert!(at(10.8) <= 1e-6, "{}", at(10.8));
    }

    #[test]
    fn interpolation_between_frames() {
        let cv = LevelerCurve {
            gain_db: vec![0.0, 4.0, 4.0],
            hop: 4,
            first_center: -2,
            stats: LevelerStats::default(),
        };
        assert_eq!(cv.gain_db_at(-10), 0.0);
        assert_eq!(cv.gain_db_at(0), 2.0);
        assert_eq!(cv.gain_db_at(2), 4.0);
        assert_eq!(cv.gain_db_at(100), 4.0);
    }
}
