//! Akzeptanztests Phase 2: Rauschprofilsuche und Rauschreduktion.
//!
//! Sprache und Rauschen werden getrennt synthetisiert und gemischt. Für die
//! Qualitätsmetriken werden die Gain-Masken des Denoisers aufgezeichnet und
//! getrennt auf Sprache und Rauschen angewendet („Schattenfilterung“). So sind
//! Sprachanteil und Restrauschen der Ausgabe exakt bekannt.

use leveled_audio_analysis::{FrameClass, ProfileBuilder, ProfileSource, ProfileStatistic};
use leveled_audio_dsp::{DenoiseParams, GainRule, SpectralDenoiser, StftEngine, auto_fft_size};
use leveled_audio_io::SampleFormat;
use leveled_audio_pipeline::{
    CancelToken, DitherMode, JobOptions, NoProgress, Settings, learn_profile, load_profile,
    process_file, save_profile,
};
use leveled_audio_testkit::{
    Planar, Rng, SpeechSignal, db_to_amp, pink_noise, power_kurtosis, read_wav, synth_speech,
    temp_dir, white_noise, write_wav,
};
use std::path::Path;

const RATE: u32 = 48_000;
const SECS: f64 = 20.0;

// ------------------------------------------------------------ Signale

#[derive(Clone, Copy, Debug)]
enum NoiseKind {
    White,
    Pink,
    /// Rosa Rauschen plus Lüfter-/Brummtöne.
    Fan,
}

fn noise(kind: NoiseKind, rms_dbfs: f64, secs: f64, seed: u64) -> Vec<f32> {
    match kind {
        NoiseKind::White => white_noise(RATE, 1, rms_dbfs, secs, seed).remove(0),
        NoiseKind::Pink => pink_noise(RATE, 1, rms_dbfs, secs, seed).remove(0),
        NoiseKind::Fan => {
            let mut p = pink_noise(RATE, 1, rms_dbfs - 1.0, secs, seed).remove(0);
            let a = db_to_amp(rms_dbfs - 10.0);
            for (i, v) in p.iter_mut().enumerate() {
                let t = i as f64 / RATE as f64;
                let tones = (2.0 * std::f64::consts::PI * 120.0 * t).sin()
                    + 0.5 * (2.0 * std::f64::consts::PI * 240.0 * t).sin()
                    + 0.3 * (2.0 * std::f64::consts::PI * 1_130.0 * t).sin();
                *v += (a * tones) as f32;
            }
            p
        }
    }
}

struct Scene {
    speech: SpeechSignal,
    noise: Vec<f32>,
    noisy: Vec<f32>,
}

/// Sprache bei −23 dBFS (aktive Abschnitte), Rauschen mit `snr_db` Abstand.
fn scene(kind: NoiseKind, snr_db: f64, seed: u64) -> Scene {
    let speech = synth_speech(RATE, SECS, 115.0, -23.0, seed);
    let noise = noise(kind, -23.0 - snr_db, SECS, seed + 100);
    let noisy = speech
        .samples
        .iter()
        .zip(&noise)
        .map(|(a, b)| a + b)
        .collect();
    Scene {
        speech,
        noise,
        noisy,
    }
}

fn write_mono(path: &Path, x: &[f32]) {
    write_wav(path, RATE, SampleFormat::Float32, &vec![x.to_vec()]);
}

// ------------------------------------------------------------ Hilfen

/// Mittlere Leistung je Bin über alle Frames (Referenz-PSD des Rauschens).
fn true_noise_power(noise: &[f32], n: usize) -> Vec<f32> {
    let mut eng = StftEngine::new(1, n, false);
    let mut b = ProfileBuilder::new(1, n / 2 + 1);
    let mut skip = 4; // Einschwingen (Frames mit Nullvorlauf)
    eng.process(&[noise.to_vec()], noise.len(), &mut [], |f| {
        if skip > 0 {
            skip -= 1;
        } else {
            b.push(f.spectra);
        }
    });
    b.finish(ProfileStatistic::Mean, RATE, n).0.remove(0)
}

/// Terzbänder 100 Hz … 16 kHz: größte Abweichung in dB.
fn max_band_error_db(est: &[f32], truth: &[f32], n: usize) -> f64 {
    let mut worst = 0f64;
    let mut fc = 100.0f64;
    while fc <= 16_000.0 {
        let lo = (fc / 2f64.powf(1.0 / 6.0) * n as f64 / RATE as f64).floor() as usize;
        let hi = ((fc * 2f64.powf(1.0 / 6.0) * n as f64 / RATE as f64).ceil() as usize).max(lo + 1);
        let e: f64 = est[lo..hi].iter().map(|v| *v as f64).sum();
        let t: f64 = truth[lo..hi].iter().map(|v| *v as f64).sum();
        let d = 10.0 * (e / t).log10();
        if std::env::var("VB_DEBUG").is_ok() {
            eprint!("{fc:.0}:{d:+.1} ");
        }
        worst = worst.max(d.abs());
        fc *= 2f64.powf(1.0 / 3.0);
    }
    worst
}

fn learn(path: &Path, settings: &Settings) -> leveled_audio_analysis::NoiseProfile {
    learn_profile(path, settings, &NoProgress, &CancelToken::new())
        .unwrap()
        .0
}

/// Aufgezeichnete Gain-Masken auf Sprache und Rauschen getrennt anwenden.
/// Liefert (Sprachanteil, Restrauschen) der Ausgabe, latenzbereinigt.
fn shadow_filter(sc: &Scene, power: &[f32], p: DenoiseParams) -> (Vec<f32>, Vec<f32>) {
    let n = auto_fft_size(RATE);
    let len = sc.noisy.len();
    let pad = |x: &[f32]| {
        let mut v = x.to_vec();
        v.extend(std::iter::repeat_n(0.0, n));
        vec![v]
    };
    let mut den = SpectralDenoiser::new(&[power.to_vec()], (n / 4) as f64 / RATE as f64, p);
    let mut eng = StftEngine::new(1, n, true);
    let mut gains: Vec<Vec<f32>> = Vec::new();
    let mut out = vec![Vec::new()];
    eng.process(&pad(&sc.noisy), len + n, &mut out, |f| {
        den.process(f.spectra);
        gains.push(den.last_gain().to_vec());
    });
    let apply = |x: &[f32]| {
        let mut eng = StftEngine::new(1, n, true);
        let mut out = vec![Vec::new()];
        eng.process(&pad(x), len + n, &mut out, |f| {
            for (v, g) in f.spectra[0].iter_mut().zip(&gains[f.index as usize]) {
                *v *= *g;
            }
        });
        out.remove(0)[n..].to_vec()
    };
    (apply(&sc.speech.samples), apply(&sc.noise))
}

/// Sample-Masken: aktive Sprache bzw. sichere Pause (≥ 150 ms von Sprache).
fn masks(sp: &SpeechSignal) -> (Vec<bool>, Vec<bool>) {
    let len = sp.samples.len();
    let mut active = vec![false; len];
    let mut near = vec![false; len];
    let guard = (0.15 * RATE as f64) as usize;
    for &(s, e) in &sp.active {
        active[s..e].iter_mut().for_each(|v| *v = true);
        near[s.saturating_sub(guard)..(e + guard).min(len)]
            .iter_mut()
            .for_each(|v| *v = true);
    }
    let pause = near.iter().map(|v| !v).collect();
    (active, pause)
}

fn energy(x: &[f32], mask: &[bool]) -> f64 {
    x.iter()
        .zip(mask)
        .filter(|(_, m)| **m)
        .map(|(v, _)| (*v as f64).powi(2))
        .sum()
}

/// Log-Kurtosis-Ratio des Restrauschens in Pausen (Musical-Noise-Maß).
fn log_kurtosis_ratio(before: &[f32], after: &[f32], pause: &[bool]) -> f64 {
    let n = 1024;
    let hop = n / 4;
    let collect = |x: &[f32]| {
        let mut eng = StftEngine::new(1, n, false);
        let mut vals = Vec::new();
        eng.process(&[x.to_vec()], x.len(), &mut [], |f| {
            let end = (f.index as usize + 1) * hop;
            if end >= n && pause[end - n..end].iter().all(|v| *v) {
                // 200 Hz … 8 kHz
                for v in &f.spectra[0][4..170] {
                    vals.push(v.norm_sqr() as f64);
                }
            }
        });
        vals
    };
    (power_kurtosis(&collect(after)) / power_kurtosis(&collect(before))).ln()
}

// ------------------------------------------------------------ Tests

#[test]
fn profile_matches_known_noise_spectrum() {
    let dir = temp_dir("profile-accuracy");
    let n = auto_fft_size(RATE);
    for kind in [NoiseKind::White, NoiseKind::Pink, NoiseKind::Fan] {
        let sc = scene(kind, 20.0, 11);
        let path = dir.join("noisy.wav");
        write_mono(&path, &sc.noisy);
        let prof = learn(&path, &Settings::default());
        assert_eq!(prof.source, ProfileSource::Auto, "{kind:?}");
        assert!(
            prof.confidence > 0.7,
            "{kind:?}: Konfidenz {}",
            prof.confidence
        );
        let est = prof.power_for(RATE, n, 1).remove(0);
        let err = max_band_error_db(&est, &true_noise_power(&sc.noise, n), n);
        eprintln!("{kind:?}: max. Terzband-Abweichung {err:.2} dB");
        assert!(err <= 1.5, "{kind:?}: Terzband-Abweichung {err:.2} dB");
    }
}

#[test]
fn profile_robust_against_silence_fades_breaths_and_clicks() {
    let dir = temp_dir("profile-robust");
    let n = auto_fft_size(RATE);
    let sc = scene(NoiseKind::Pink, 25.0, 21);
    let mut x = sc.noisy.clone();
    let len = x.len();
    // Digitale Stille am Anfang, dann Fade-in; Fade-out ans Ende.
    let sil = (0.8 * RATE as f64) as usize;
    let fade = (0.3 * RATE as f64) as usize;
    x[..sil].iter_mut().for_each(|v| *v = 0.0);
    for i in 0..fade {
        x[sil + i] *= i as f32 / fade as f32;
        x[len - 1 - i] *= i as f32 / fade as f32;
    }
    // Atemartige Bursts (+15 dB, 250 ms) und Klicks in den Pausen.
    let (_, pause) = masks(&sc.speech);
    let mut rng = Rng::new(5);
    let breath_amp = db_to_amp(-23.0 - 25.0 + 15.0) as f32;
    let blen = (0.25 * RATE as f64) as usize;
    let mut placed = 0;
    let mut i = sil + fade;
    while i + blen < len - fade && placed < 6 {
        if pause[i..i + blen].iter().all(|v| *v) {
            for k in 0..blen {
                let env = (std::f32::consts::PI * k as f32 / blen as f32).sin();
                x[i + k] += breath_amp * env * rng.uniform() as f32;
            }
            x[i + blen + 2_000] += 0.3; // Klick
            placed += 1;
            i += 3 * RATE as usize;
        } else {
            i += 2_400;
        }
    }
    assert!(placed >= 3, "zu wenige Bursts platziert ({placed})");
    let path = dir.join("noisy.wav");
    write_mono(&path, &x);
    let prof = learn(&path, &Settings::default());
    assert_eq!(prof.source, ProfileSource::Auto);
    let est = prof.power_for(RATE, n, 1).remove(0);
    let err = max_band_error_db(&est, &true_noise_power(&sc.noise, n), n);
    eprintln!("robust: max. Terzband-Abweichung {err:.2} dB");
    assert!(err <= 1.5, "Terzband-Abweichung {err:.2} dB");
}

#[test]
fn continuous_speech_falls_back_to_minimum_statistics() {
    let dir = temp_dir("profile-fallback");
    let n = auto_fft_size(RATE);
    // Fließende Rede: Wortlücken bleiben, jede längere Lücke wird auf 120 ms
    // gekürzt – damit gibt es keine Pause, die die VAD als Pause stehen lässt.
    let sp = synth_speech(RATE, 40.0, 130.0, -23.0, 31);
    let max_gap = (0.12 * RATE as f64) as usize;
    let mut speech: Vec<f32> = Vec::new();
    for (i, &(s, e)) in sp.active.iter().enumerate() {
        speech.extend_from_slice(&sp.samples[s..e]);
        if let Some(&(next, _)) = sp.active.get(i + 1) {
            speech.extend(std::iter::repeat_n(0.0, (next - e).min(max_gap)));
        }
    }
    speech.truncate((15.0 * RATE as f64) as usize);
    let nz = noise(
        NoiseKind::Pink,
        -23.0 - 20.0,
        speech.len() as f64 / RATE as f64,
        9,
    );
    let x: Vec<f32> = speech.iter().zip(&nz).map(|(a, b)| a + b).collect();
    let path = dir.join("noisy.wav");
    write_mono(&path, &x);
    let prof = learn(&path, &Settings::default());
    assert_eq!(prof.source, ProfileSource::Minstat);
    let est = prof.power_for(RATE, n, 1).remove(0);
    let truth = true_noise_power(&nz, n);
    let lvl = |p: &[f32]| 10.0 * p.iter().map(|v| *v as f64).sum::<f64>().log10();
    let diff = lvl(&est) - lvl(&truth);
    eprintln!("Minimum Statistics: Pegelabweichung {diff:+.2} dB");
    assert!(diff.abs() <= 3.0, "Gesamtpegel-Abweichung {diff:.2} dB");
}

#[test]
fn denoise_quality_at_20db_snr() {
    let dir = temp_dir("denoise-quality");
    let n = auto_fft_size(RATE);
    let sc = scene(NoiseKind::Pink, 20.0, 41);
    let path = dir.join("noisy.wav");
    write_mono(&path, &sc.noisy);
    let power = learn(&path, &Settings::default())
        .power_for(RATE, n, 1)
        .remove(0);
    let (active, pause) = masks(&sc.speech);
    let all = vec![true; sc.noisy.len()];

    let mut results = Vec::new();
    for (rule, reduction) in [
        (GainRule::WienerDd, 12.0),
        (GainRule::Subtract, 12.0),
        (GainRule::WienerDd, 20.0),
    ] {
        let p = DenoiseParams {
            gain_rule: rule,
            reduction_db: reduction,
            ..Default::default()
        };
        let (out_speech, out_noise) = shadow_filter(&sc, &power, p);
        let att_pause = 10.0 * (energy(&out_noise, &pause) / energy(&sc.noise, &pause)).log10();
        let snr_in = 10.0 * (energy(&sc.speech.samples, &all) / energy(&sc.noise, &all)).log10();
        let snr_out = 10.0 * (energy(&out_speech, &all) / energy(&out_noise, &all)).log10();
        let diff: Vec<f32> = sc
            .speech
            .samples
            .iter()
            .zip(&out_speech)
            .map(|(a, b)| a - b)
            .collect();
        let distortion =
            10.0 * (energy(&sc.speech.samples, &active) / energy(&diff, &active)).log10();
        let lkr = log_kurtosis_ratio(&sc.noise, &out_noise, &pause);
        eprintln!(
            "{rule:?} {reduction} dB: Pausen {att_pause:.1} dB, SNR {snr_in:.1} → {snr_out:.1} dB (+{:.1}), \
             Sprachverzerrung {distortion:.1} dB, Log-Kurtosis-Ratio {lkr:.3}",
            snr_out - snr_in
        );
        results.push((rule, att_pause, snr_out - snr_in, distortion, lkr));
    }
    let (_, att, gain, dist, lkr_dd) = results[0];
    // Bei max. 12 dB Absenkung bleibt Rauschen in sprachdominierten Bins
    // während der Sprache stehen (gewollt). Der Gesamt-SNR-Gewinn ist damit
    // begrenzt: ≈ +6 dB bei 20 dB, ≈ +8 dB bei 10 dB Eingangs-SNR.
    assert!(att <= -11.0, "Rauschdämpfung in Pausen nur {att:.1} dB");
    assert!(gain >= 5.0, "SNR-Gewinn nur {gain:.1} dB");
    assert!(dist >= 20.0, "Sprachverzerrung zu hoch ({dist:.1} dB)");
    let lkr_sub = results[1].4;
    assert!(
        lkr_dd < lkr_sub,
        "wiener-dd sollte weniger Musical Noise erzeugen ({lkr_dd:.3} vs. {lkr_sub:.3})"
    );
    // Regressionsschranke (Basiswert bei Einführung ≈ 0,1).
    assert!(lkr_dd < 0.3, "Log-Kurtosis-Ratio {lkr_dd:.3}");
    // Mehr Absenkung wirkt (Mechanik), auch wenn sie nicht Default ist.
    assert!(results[2].1 < att - 3.0 && results[2].2 > gain);
}

#[test]
fn delta_plus_output_reconstructs_input() {
    let dir = temp_dir("delta");
    let sc = scene(NoiseKind::White, 15.0, 51);
    let stereo: Planar = vec![
        sc.noisy.clone(),
        sc.noisy.iter().map(|v| -0.5 * v).collect(),
    ];
    let input = dir.join("in.wav");
    write_wav(&input, RATE, SampleFormat::Float32, &stereo);
    let mut s = Settings::default();
    s.loudness.enabled = false;
    s.limiter.enabled = false;
    s.output.dither = DitherMode::Off;
    let opts = JobOptions {
        export_delta: true,
        ..Default::default()
    };
    let run = |s: &Settings| {
        let out = dir.join("out.wav");
        let r = process_file(&input, &out, s, &opts, 0, &NoProgress, &CancelToken::new()).unwrap();
        let delta = r.denoise.as_ref().unwrap().delta_path.clone().unwrap();
        (read_wav(&out).1, read_wav(&delta).1)
    };
    let (out, delta) = run(&s);
    let mut max_err = 0f32;
    let mut removed = 0f64;
    for c in 0..2 {
        assert_eq!(out[c].len(), stereo[c].len());
        for i in 0..stereo[c].len() {
            max_err = max_err.max((out[c][i] + delta[c][i] - stereo[c][i]).abs());
            removed += (delta[c][i] as f64).powi(2);
        }
    }
    assert!(max_err < 1e-5, "Ausgabe + Delta ≠ Eingabe ({max_err})");
    assert!(removed > 0.0);

    // Ohne Absenkung ist das Delta (bis auf Rundung) null.
    s.denoise.reduction_db = 0.0;
    let (out, delta) = run(&s);
    let peak = delta.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    assert!(20.0 * (peak as f64).log10() < -120.0, "Delta-Peak {peak}");
    assert_eq!(out[0].len(), stereo[0].len());
}

#[test]
fn profile_export_import_and_forced_profile() {
    let dir = temp_dir("profile-io");
    let sc = scene(NoiseKind::Pink, 20.0, 61);
    let a = dir.join("a.wav");
    write_mono(&a, &sc.noisy);
    let prof = learn(&a, &Settings::default());
    let json = dir.join("p.json");
    save_profile(&json, &prof).unwrap();
    let back = load_profile(&json).unwrap();
    assert_eq!(back, prof);

    // Erzwungenes Profil auf eine Stereo-Datei mit 44,1 kHz.
    let b = dir.join("b.wav");
    let x44: Vec<f32> = sc.noisy.iter().step_by(2).copied().collect(); // nur Testmaterial
    write_wav(&b, 44_100, SampleFormat::Int24, &vec![x44.clone(), x44]);
    let opts = JobOptions {
        noise_profile: Some(std::sync::Arc::new(back)),
        ..Default::default()
    };
    let r = process_file(
        &b,
        &dir.join("b_out.wav"),
        &Settings::default(),
        &opts,
        0,
        &NoProgress,
        &CancelToken::new(),
    )
    .unwrap();
    let d = r.denoise.unwrap();
    assert_eq!(d.profile_source, Some(ProfileSource::Imported));
    assert!(d.skipped.is_none());

    // Kaputtes Profil wird sauber abgelehnt.
    std::fs::write(&json, "{\"format_version\": 1}").unwrap();
    assert!(load_profile(&json).is_err());
}

#[test]
fn vad_finds_synthetic_speech() {
    // VAD gegen die bekannte Aktivität: ≥ 95 % der aktiven Frames als Sprache,
    // ≥ 80 % der sicheren Pausen als Pause.
    use leveled_audio_analysis::{FeatureExtractor, VadParams, classify};
    let sc = scene(NoiseKind::Pink, 20.0, 71);
    let n = auto_fft_size(RATE);
    let hop = n / 4;
    let mut eng = StftEngine::new(1, n, false);
    let mut fx = FeatureExtractor::new(RATE, n, 1);
    eng.process(
        std::slice::from_ref(&sc.noisy),
        sc.noisy.len(),
        &mut [],
        |f| fx.push(f.spectra, f.time),
    );
    let f = fx.finish();
    let cls = classify(&f, &VadParams::default());
    let (active, pause) = masks(&sc.speech);
    let (mut a_tot, mut a_hit, mut p_tot, mut p_hit) = (0, 0, 0, 0);
    for (k, c) in cls.iter().enumerate() {
        let centre = ((k + 1) * hop).saturating_sub(n / 2);
        if centre >= active.len() {
            break;
        }
        if active[centre] {
            a_tot += 1;
            a_hit += (*c == FrameClass::Speech) as usize;
        } else if pause[centre] {
            p_tot += 1;
            p_hit += (*c == FrameClass::Pause) as usize;
        }
    }
    let a = a_hit as f64 / a_tot as f64;
    let p = p_hit as f64 / p_tot as f64;
    assert!(a >= 0.95, "Sprache erkannt: {:.1} %", a * 100.0);
    assert!(p >= 0.80, "Pausen erkannt: {:.1} %", p * 100.0);
}
