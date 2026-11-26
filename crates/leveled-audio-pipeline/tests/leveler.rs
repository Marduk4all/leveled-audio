//! Akzeptanztests Phase 3: Adaptiver Leveler.
//!
//! Zwei synthetische Sprecher mit bekanntem Pegelunterschied wechseln sich ab.
//! Rauschreduktion, Normalisierung und Limiter sind für die Messungen aus und
//! die Ausgabe ist 32 bit float: Dann ist `Ausgabe / Eingabe` je Sample exakt
//! der angewendete Leveler-Gain.

use leveled_audio_io::SampleFormat;
use leveled_audio_pipeline::{
    BitDepth, CancelToken, DitherMode, FileReport, JobOptions, NoProgress, Settings, process_file,
};
use leveled_audio_testkit::{pink_noise, read_wav, synth_speech, temp_dir, write_wav};
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;

struct Dialogue {
    samples: Vec<f32>,
    /// Je Sprecherwechsel: (Sprecher 0/1, aktive Sample-Bereiche).
    turns: Vec<(usize, Vec<(usize, usize)>)>,
    /// Lange Pausen als Sample-Bereiche.
    long_pauses: Vec<(usize, usize)>,
}

/// Abwechselnd Sprecher A (−20 dBFS) und B (−20 − `diff_db`), je 5 s, mit
/// 1 s Pause; nach dem 4. Wechsel eine lange Pause (3 s). Rosa Rauschen −70 dBFS.
fn dialogue(turns: usize, diff_db: f64) -> Dialogue {
    let gap = RATE as usize;
    let mut samples = vec![0f32; gap];
    let mut out_turns = Vec::new();
    let mut long_pauses = Vec::new();
    for t in 0..turns {
        let who = t % 2;
        let (f0, level) = if who == 0 {
            (115.0, -20.0)
        } else {
            (210.0, -20.0 - diff_db)
        };
        let sp = synth_speech(RATE, 5.0, f0, level, 100 + t as u64);
        let off = samples.len();
        out_turns.push((
            who,
            sp.active.iter().map(|&(a, b)| (a + off, b + off)).collect(),
        ));
        samples.extend_from_slice(&sp.samples);
        let pause = if t == 3 { 3 * gap } else { gap };
        if t == 3 {
            long_pauses.push((samples.len(), samples.len() + pause));
        }
        samples.extend(std::iter::repeat_n(0.0, pause));
    }
    let noise = pink_noise(RATE, 1, -70.0, samples.len() as f64 / RATE as f64 + 0.1, 5);
    for (s, n) in samples.iter_mut().zip(&noise[0]) {
        *s += n;
    }
    Dialogue {
        samples,
        turns: out_turns,
        long_pauses,
    }
}

fn leveler_only(strength: f64) -> Settings {
    let mut s = Settings::default();
    s.denoise.enabled = false;
    s.loudness.enabled = false;
    s.limiter.enabled = false;
    s.output.bit_depth = BitDepth::F32;
    s.output.dither = DitherMode::Off;
    s.highpass.enabled = false;
    s.compressor.enabled = false;
    // Die synthetische Stimme ist unrealistisch tiefenlastig (Sprachband ≈ 16 dB
    // unter den Tiefen; echte Sprache −14 … +10 dB, Median ≈ 0) und würde als
    // Störgeräusch erkannt. Der Leveler wird daher ohne Störgeräusch-Erkennung
    // geprüft; die Erkennung hat eigene Tests (tests/phase5.rs).
    s.knock.enabled = false;
    s.leveler.strength = strength;
    s
}

fn run(input: &Path, output: &Path, s: &Settings) -> FileReport {
    process_file(
        input,
        output,
        s,
        &JobOptions::default(),
        0,
        &NoProgress,
        &CancelToken::new(),
    )
    .unwrap()
}

fn write_input(dir: &Path, d: &Dialogue) -> PathBuf {
    let p = dir.join("dialog.wav");
    write_wav(&p, RATE, SampleFormat::Float32, &vec![d.samples.clone()]);
    p
}

fn rms_db(x: &[f32], ranges: &[(usize, usize)]) -> f64 {
    let (mut sum, mut n) = (0f64, 0usize);
    for &(a, b) in ranges {
        for v in &x[a..b] {
            sum += (*v as f64).powi(2);
        }
        n += b - a;
    }
    10.0 * (sum / n as f64).log10()
}

/// Pegelunterschied A − B (Energiemittel über alle Wechsel eines Sprechers).
fn speaker_diff(x: &[f32], d: &Dialogue) -> f64 {
    let level = |who: usize| {
        let ranges: Vec<(usize, usize)> = d
            .turns
            .iter()
            .filter(|t| t.0 == who)
            .flat_map(|t| t.1.iter().copied())
            .collect();
        rms_db(x, &ranges)
    };
    level(0) - level(1)
}

/// Gain je Sample (dB) aus Ausgabe/Eingabe, nur wo die Eingabe deutlich ≠ 0.
fn gain_db(input: &[f32], output: &[f32], i: usize) -> Option<f64> {
    (input[i].abs() > 1e-3).then(|| 20.0 * (output[i] as f64 / input[i] as f64).log10())
}

#[test]
fn equalizes_two_speakers_10db_apart() {
    let dir = temp_dir("leveler-speakers");
    let d = dialogue(8, 10.0);
    let input = write_input(&dir, &d);
    let before = speaker_diff(&d.samples, &d);
    assert!((before - 10.0).abs() < 0.3, "Eingangsdifferenz {before:.2}");

    let mut diffs = Vec::new();
    for strength in [1.0, 0.5] {
        let out = dir.join(format!("out_{strength}.wav"));
        let r = run(&input, &out, &leveler_only(strength));
        let y = read_wav(&out).1.remove(0);
        let diff = speaker_diff(&y, &d);
        let lev = r.leveler.unwrap();
        eprintln!(
            "Stärke {strength}: Differenz {diff:.2} dB, Phrasen {}, Gain {:+.1}…{:+.1}",
            lev.phrases, lev.gain_min_db, lev.gain_max_db
        );
        diffs.push(diff);
    }
    assert!(diffs[0].abs() < 2.0, "100 %: {:.2} dB", diffs[0]);
    assert!((diffs[1] - 5.0).abs() <= 1.0, "50 %: {:.2} dB", diffs[1]);
}

#[test]
fn no_pumping_within_phrases_and_quiet_long_pauses() {
    let dir = temp_dir("leveler-pumping");
    let d = dialogue(6, 10.0);
    let input = write_input(&dir, &d);
    let out = dir.join("out.wav");
    run(&input, &out, &leveler_only(1.0));
    let y = read_wav(&out).1.remove(0);
    let x = &d.samples;

    // Phrasen: aktive Bereiche, die weniger als 350 ms auseinanderliegen.
    let mut worst = 0f64;
    for (_, ranges) in &d.turns {
        let mut phrases: Vec<Vec<(usize, usize)>> = Vec::new();
        for &r in ranges {
            match phrases.last_mut() {
                Some(p) if r.0 - p.last().unwrap().1 < RATE as usize * 35 / 100 => p.push(r),
                _ => phrases.push(vec![r]),
            }
        }
        for p in phrases {
            let (lo, hi) = p
                .iter()
                .flat_map(|&(a, b)| a..b)
                .filter_map(|i| gain_db(x, &y, i))
                .fold((f64::MAX, f64::MIN), |(l, h), g| (l.min(g), h.max(g)));
            if hi >= lo {
                worst = worst.max(hi - lo);
            }
        }
    }
    eprintln!("größte Gain-Schwankung innerhalb einer Phrase: {worst:.2} dB");
    assert!(worst < 1.0, "Pumpen: {worst:.2} dB");

    // Inneres der langen Pause (≥ 0,7 s Abstand zur Sprache): nicht angehoben.
    // Das Rauschen liegt bei −70 dBFS; der Gain wird über Energien geschätzt.
    let margin = RATE as usize * 7 / 10;
    for &(a, b) in &d.long_pauses {
        let inner = (a + margin, b - margin);
        let g = rms_db(&y, &[inner]) - rms_db(x, &[inner]);
        assert!(g <= 0.05, "Gain in langer Pause {g:+.2} dB");
    }
}

#[test]
fn zero_strength_is_bit_identical_to_bypass() {
    let dir = temp_dir("leveler-zero");
    let d = dialogue(4, 8.0);
    let input = write_input(&dir, &d);
    for denoise in [true, false] {
        let mut off = Settings::default();
        off.knock.enabled = false;
        off.denoise.enabled = denoise;
        off.leveler.enabled = false;
        let mut zero = off.clone();
        zero.leveler.enabled = true;
        zero.leveler.strength = 0.0;
        run(&input, &dir.join("a.wav"), &off);
        run(&input, &dir.join("b.wav"), &zero);
        let a = std::fs::read(dir.join("a.wav")).unwrap();
        let b = std::fs::read(dir.join("b.wav")).unwrap();
        assert_eq!(a, b, "Rauschreduktion {denoise}");
    }
}

#[test]
fn full_chain_with_leveler_hits_target() {
    let dir = temp_dir("leveler-chain");
    let d = dialogue(6, 10.0);
    let input = write_input(&dir, &d);
    let out = dir.join("out.wav");

    // Ohne Limiter: Pass 2 misst nach dem Leveler, das Ziel wird exakt getroffen.
    let mut s = Settings::default();
    s.knock.enabled = false;
    s.limiter.enabled = false;
    s.output.bit_depth = BitDepth::F32;
    let r = run(&input, &out, &s);
    let i = r.loudness_out.unwrap().integrated_lufs.unwrap();
    assert!((i + 16.0).abs() <= 0.1, "ohne Limiter {i:.2} LUFS");

    // Volle Kette: True Peak eingehalten, Ziel auf ±0,3 LU (Korrekturschwelle).
    // Die synthetische Stimme ist sehr spitz; seit Phase 5 fängt der Kompressor
    // einen Teil ab, sonst greift die Korrekturiteration.
    let mut s = Settings::default();
    s.knock.enabled = false;
    let r = run(&input, &out, &s);
    let o = r.loudness_out.unwrap();
    let i = o.integrated_lufs.unwrap();
    eprintln!(
        "{i:.2} LUFS, Korrektur {:?}, Limiter {:?}, Leveler {:?}",
        r.loudness_correction_db, r.limiter, r.leveler
    );
    assert!((i + 16.0).abs() <= 0.3, "{i:.2} LUFS");
    assert!(
        r.warnings.iter().all(|w| !w.contains("verfehlt")),
        "{:?}",
        r.warnings
    );
    assert!(o.true_peak_dbtp.unwrap() <= -1.0);
    let lev = r.leveler.unwrap();
    assert!(lev.phrases >= 6, "{} Phrasen", lev.phrases);
    assert!(lev.gain_max_db > 2.0 && lev.gain_min_db < -2.0, "{lev:?}");
    assert_eq!(read_wav(&out).1[0].len(), d.samples.len());
}
