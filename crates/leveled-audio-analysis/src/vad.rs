//! Sprach-/Pausenerkennung über die Feature-Arrays (offline, mit Vorausschau).
//!
//! 1. Hintergrundpegel `B`: gleitendes Perzentil von `e_db` (ohne digitale Stille).
//! 2. Aktiv, wenn `e > B + on_db`; Halten bis `e < B + off_db` (Hysterese).
//! 3. Ein aktiver Bereich ist Sprache, wenn er lang genug ist und genügend
//!    tonale (stimmhafte) Frames enthält; sonst „Other“ (Klick, Atmer, Geräusch).
//! 4. Lücken zwischen Sprachbereichen werden geschlossen, Pre-Roll und
//!    Hangover angehängt. Der Rest ist Pause.

use crate::features::FrameFeatures;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FrameClass {
    Speech,
    Pause,
    /// Aktiv, aber keine Sprache (Transient, Atmer, Geräusch).
    Other,
    /// Digitale Stille (Nullen, eingefrorene Samples).
    Silence,
    /// Aktiv, aber kaum Energie im Sprachband: tieffrequente Störung wie
    /// Griffgeräusche oder Rumpeln (PLAN 3.13). Wird nicht gelevelt und in der
    /// Rauschreduktion in den Tiefen abgesenkt.
    Noise,
}

#[derive(Debug, Clone, Copy)]
pub struct VadParams {
    pub on_db: f32,
    pub off_db: f32,
    /// Halbe Fensterbreite der Hintergrundschätzung in Sekunden.
    pub background_half_window_s: f64,
    pub background_percentile: f64,
    pub hangover_ms: f64,
    pub preroll_ms: f64,
    pub min_speech_ms: f64,
    pub close_gap_ms: f64,
    /// Frames mit Flachheit unter diesem Wert gelten als stimmhaft.
    pub voiced_flatness: f32,
    pub min_voiced_frames: usize,
    pub silence_db: f32,
    /// Aktive Frames, deren Sprachband (300–3400 Hz) um mehr als diesen Wert
    /// unter den Tiefen (20–300 Hz) liegt, gelten als Störgeräusch
    /// (`FrameClass::Noise`). `None` = keine Störgeräusch-Erkennung.
    pub noise_max_speech_to_low_db: Option<f32>,
    /// Lücken zwischen Störgeräusch-Frames bis zu dieser Dauer werden geschlossen.
    pub noise_close_gap_ms: f64,
    /// Kürzere Störgeräusch-Bereiche innerhalb von Sprache bleiben Sprache
    /// (einzelne tiefe Frames, z. B. Nasale, sollen Phrasen nicht teilen).
    pub noise_min_ms: f64,
}

impl Default for VadParams {
    fn default() -> Self {
        VadParams {
            on_db: 8.0,
            off_db: 5.0,
            background_half_window_s: 3.0,
            background_percentile: 0.10,
            hangover_ms: 150.0,
            preroll_ms: 50.0,
            min_speech_ms: 60.0,
            close_gap_ms: 250.0,
            voiced_flatness: 0.4,
            min_voiced_frames: 3,
            silence_db: -90.0,
            noise_max_speech_to_low_db: Some(-15.0),
            noise_close_gap_ms: 70.0,
            noise_min_ms: 40.0,
        }
    }
}

const HIST_MIN: f32 = -120.0;
const HIST_STEP: f32 = 0.5;
const HIST_BINS: usize = 241;

fn hist_bin(e: f32) -> usize {
    (((e - HIST_MIN) / HIST_STEP).round().max(0.0) as usize).min(HIST_BINS - 1)
}

/// Gleitendes Perzentil über ±`half` Frames, Frames mit `skip[k]` ausgenommen.
pub fn sliding_percentile(values: &[f32], skip: &[bool], half: usize, q: f64) -> Vec<f32> {
    let n = values.len();
    let mut hist = [0u32; HIST_BINS];
    let mut count = 0u32;
    let mut out = Vec::with_capacity(n);
    let add = |hist: &mut [u32; HIST_BINS], count: &mut u32, i: usize, sign: i32| {
        if !skip[i] {
            let b = hist_bin(values[i]);
            if sign > 0 {
                hist[b] += 1;
                *count += 1;
            } else {
                hist[b] -= 1;
                *count -= 1;
            }
        }
    };
    for i in 0..half.min(n) {
        add(&mut hist, &mut count, i, 1);
    }
    for k in 0..n {
        if k + half < n {
            add(&mut hist, &mut count, k + half, 1);
        }
        if k > half {
            add(&mut hist, &mut count, k - half - 1, -1);
        }
        if count == 0 {
            out.push(HIST_MIN);
            continue;
        }
        let target = ((count as f64 * q).floor() as u32).min(count - 1);
        let mut acc = 0u32;
        let mut val = HIST_MIN;
        for (b, &h) in hist.iter().enumerate() {
            acc += h;
            if acc > target {
                val = HIST_MIN + b as f32 * HIST_STEP;
                break;
            }
        }
        out.push(val);
    }
    out
}

pub fn classify(f: &FrameFeatures, p: &VadParams) -> Vec<FrameClass> {
    let n = f.len();
    let ms = |m: f64| f.frames_for(m / 1000.0);
    let silence: Vec<bool> = (0..n)
        .map(|k| f.flat_frac[k] > 0.5 || f.e_db[k] < p.silence_db)
        .collect();
    let bg = sliding_percentile(
        &f.e_db,
        &silence,
        f.frames_for(p.background_half_window_s),
        p.background_percentile,
    );

    // Aktive Bereiche mit Hysterese.
    let mut active = vec![false; n];
    let mut on = false;
    for k in 0..n {
        if silence[k] {
            on = false;
            continue;
        }
        let rel = f.e_db[k] - bg[k];
        on = if on { rel > p.off_db } else { rel > p.on_db };
        active[k] = on;
    }

    let mut class: Vec<FrameClass> = (0..n)
        .map(|k| {
            if silence[k] {
                FrameClass::Silence
            } else if active[k] {
                FrameClass::Other
            } else {
                FrameClass::Pause
            }
        })
        .collect();

    // Aktive Bereiche als Sprache einstufen.
    let min_len = ms(p.min_speech_ms);
    let mut k = 0;
    let mut speech_runs = Vec::new();
    while k < n {
        if !active[k] {
            k += 1;
            continue;
        }
        let start = k;
        while k < n && active[k] {
            k += 1;
        }
        let voiced = (start..k)
            .filter(|&i| f.flatness[i] < p.voiced_flatness)
            .count();
        if k - start >= min_len && voiced >= p.min_voiced_frames {
            speech_runs.push((start, k));
        }
    }

    // Lücken schließen, Pre-Roll und Hangover.
    let gap = ms(p.close_gap_ms);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in speech_runs {
        match merged.last_mut() {
            Some(last) if s - last.1 <= gap => last.1 = e,
            _ => merged.push((s, e)),
        }
    }
    let pre = ms(p.preroll_ms);
    let hang = ms(p.hangover_ms);
    for (s, e) in merged {
        for c in class
            .iter_mut()
            .take((e + hang).min(n))
            .skip(s.saturating_sub(pre))
        {
            if *c != FrameClass::Silence {
                *c = FrameClass::Speech;
            }
        }
    }
    if let Some(thr) = p.noise_max_speech_to_low_db {
        mark_noise(f, p, thr, &active, &mut class);
    }
    class
}

/// Störgeräusche (PLAN 3.13): aktive, tiefendominierte Frames werden `Noise`. Kurze Lücken dazwischen werden geschlossen. Sprachbereiche,
/// in denen danach keine stimmhaften Sprachband-Frames mehr übrig sind, waren
/// nur Störgeräusch samt Pre-Roll/Hangover und werden zu Pause.
fn mark_noise(
    f: &FrameFeatures,
    p: &VadParams,
    thr: f32,
    active: &[bool],
    class: &mut [FrameClass],
) {
    let n = class.len();
    let noisy = |k: usize| active[k] && f.speech_to_low_db[k] < thr;
    let speechy = |k: usize| active[k] && f.speech_to_low_db[k] >= thr;
    for k in 0..n {
        if noisy(k) && class[k] != FrameClass::Silence {
            class[k] = FrameClass::Noise;
        }
    }
    // Lücken schließen (nur über Frames ohne Sprachband-Energie).
    let gap = f.frames_for(p.noise_close_gap_ms / 1000.0);
    let mut last_noise: Option<usize> = None;
    for k in 0..n {
        if class[k] == FrameClass::Noise {
            if let Some(l) = last_noise
                && k - l > 1
                && k - l - 1 <= gap
                && (l + 1..k).all(|i| !speechy(i) && class[i] != FrameClass::Silence)
            {
                class[l + 1..k]
                    .iter_mut()
                    .for_each(|c| *c = FrameClass::Noise);
            }
            last_noise = Some(k);
        } else if speechy(k) {
            last_noise = None;
        }
    }
    // Zu kurze Störgeräusch-Bereiche zwischen Sprache zurücksetzen.
    let min_f = f.frames_for(p.noise_min_ms / 1000.0);
    let mut k = 0;
    while k < n {
        if class[k] != FrameClass::Noise {
            k += 1;
            continue;
        }
        let s = k;
        while k < n && class[k] == FrameClass::Noise {
            k += 1;
        }
        let speech_before = s > 0 && class[s - 1] == FrameClass::Speech;
        let speech_after = k < n && class[k] == FrameClass::Speech;
        if k - s < min_f && (speech_before || speech_after) {
            class[s..k].iter_mut().for_each(|c| *c = FrameClass::Speech);
        }
    }
    // Sprachbereiche ohne verbleibende stimmhafte Sprachband-Frames auflösen.
    let mut k = 0;
    while k < n {
        if class[k] != FrameClass::Speech {
            k += 1;
            continue;
        }
        let s = k;
        while k < n && class[k] == FrameClass::Speech {
            k += 1;
        }
        let voiced = (s..k)
            .filter(|&i| speechy(i) && f.flatness[i] < p.voiced_flatness)
            .count();
        if voiced < p.min_voiced_frames {
            for i in s..k {
                class[i] = if active[i] {
                    FrameClass::Other
                } else {
                    FrameClass::Pause
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_percentile_basic() {
        let v: Vec<f32> = (0..100)
            .map(|i| if i % 10 == 0 { -20.0 } else { -60.0 })
            .collect();
        let skip = vec![false; 100];
        let p = sliding_percentile(&v, &skip, 20, 0.1);
        assert!(p.iter().all(|x| (*x + 60.0).abs() < 0.5));
        let p90 = sliding_percentile(&v, &skip, 20, 0.95);
        assert!((p90[50] + 20.0).abs() < 0.5);
    }
}
