//! Akzeptanztests Phase 5: Störgeräusche (Knackser), Kompressor, Pausen-Absenkung.

use leveled_audio_io::SampleFormat;
use leveled_audio_pipeline::{
    BitDepth, CancelToken, DitherMode, FileReport, JobOptions, NoProgress, Settings, process_file,
};
use leveled_audio_testkit::{pink_noise, read_wav, synth_speech, temp_dir, write_wav};
use std::path::{Path, PathBuf};

const RATE: u32 = 48_000;

/// Sample-Bereiche `[a, b)`.
type Ranges = Vec<(usize, usize)>;

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

/// Nur die zu prüfende Stufe: kein Gain, kein Limiter, 32 bit float.
fn isolated() -> Settings {
    let mut s = Settings::default();
    s.highpass.enabled = false;
    s.leveler.enabled = false;
    s.compressor.enabled = false;
    s.gate.enabled = false;
    s.loudness.enabled = false;
    s.limiter.enabled = false;
    s.output.bit_depth = BitDepth::F32;
    s.output.dither = DitherMode::Off;
    s
}

fn write(dir: &Path, name: &str, x: &[f32]) -> PathBuf {
    let p = dir.join(name);
    write_wav(&p, RATE, SampleFormat::Float32, &vec![x.to_vec()]);
    p
}

fn rms_db(x: &[f32], a: usize, b: usize) -> f64 {
    let e = x[a..b].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / (b - a) as f64;
    10.0 * (e + 1e-30).log10()
}

fn ranges_db(x: &[f32], r: &[(usize, usize)]) -> f64 {
    let (mut e, mut n) = (0f64, 0usize);
    for &(a, b) in r {
        e += x[a..b].iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
        n += b - a;
    }
    10.0 * (e / n as f64 + 1e-30).log10()
}

/// Hochpass 2. Ordnung bei `hz` (nur zur Messung im Test).
fn highpassed(x: &[f32], hz: f64) -> Vec<f32> {
    let mut h = leveled_audio_dsp_free::Hp::new(hz, RATE as f64);
    x.iter().map(|v| h.tick(*v as f64) as f32).collect()
}

mod leveled_audio_dsp_free {
    pub struct Hp {
        b: [f64; 3],
        a: [f64; 2],
        s: [f64; 4],
    }
    impl Hp {
        pub fn new(hz: f64, rate: f64) -> Self {
            let w = 2.0 * std::f64::consts::PI * hz / rate;
            let (sn, c) = w.sin_cos();
            let al = sn / std::f64::consts::SQRT_2;
            let a0 = 1.0 + al;
            Hp {
                b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
                a: [-2.0 * c / a0, (1.0 - al) / a0],
                s: [0.0; 4],
            }
        }
        pub fn tick(&mut self, x: f64) -> f64 {
            let [x1, x2, y1, y2] = self.s;
            let y =
                self.b[0] * x + self.b[1] * x1 + self.b[2] * x2 - self.a[0] * y1 - self.a[1] * y2;
            self.s = [x, x1, y, y1];
            y
        }
    }
}

/// Sprachblöcke (auf die aktiven Bereiche zugeschnitten) mit genau den
/// angegebenen Pausen dazwischen; davor und danach 0,5 s. Liefert Signal,
/// aktive Bereiche und je Block (erste, letzte aktive Stelle).
fn blocks(f0: f64, level: f64, secs: f64, pauses: &[f64], seed: u64) -> (Vec<f32>, Ranges, Ranges) {
    let gap = |s: f64| vec![0f32; (s * RATE as f64) as usize];
    let mut x = gap(0.5);
    let mut active = Vec::new();
    let mut bounds = Vec::new();
    for (k, pause) in pauses.iter().chain(std::iter::once(&0.5)).enumerate() {
        let sp = synth_speech(RATE, secs, f0, level, seed + k as u64);
        let (a0, b0) = (sp.active.first().unwrap().0, sp.active.last().unwrap().1);
        let off = x.len() - a0;
        active.extend(sp.active.iter().map(|&(a, b)| (a + off, b + off)));
        bounds.push((a0 + off, b0 + off));
        x.extend_from_slice(&sp.samples[a0..b0]);
        x.extend(gap(*pause));
    }
    (x, active, bounds)
}

/// Griffgeräusch-Modell: gedämpfter tieffrequenter Stoß (55 Hz + 110 Hz,
/// τ = 25 ms) mit kurzem Anschlag, Spitze `peak`.
fn knock(len: usize, peak: f64) -> Vec<f32> {
    (0..len)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            let env = (-t / 0.025).exp();
            let w = 2.0 * std::f64::consts::PI;
            (peak * env * (0.8 * (w * 55.0 * t).sin() + 0.4 * (w * 110.0 * t).sin())) as f32
        })
        .collect()
}

#[test]
fn knocks_are_detected_and_attenuated_speech_untouched() {
    let dir = temp_dir("p5-knock");
    // Sprache (f0 180 Hz) in Blöcken mit 1,2-s-Pausen; Knackser mitten in den
    // Pausen und einer mitten in einer Silbe.
    let (mut x, active, bounds) = blocks(180.0, -22.0, 3.0, &[1.2, 1.2, 1.2], 21);
    let noise = pink_noise(RATE, 1, -62.0, x.len() as f64 / RATE as f64 + 0.1, 3);
    for (v, n) in x.iter_mut().zip(&noise[0]) {
        *v += n;
    }
    let sp_active = active;
    let len = RATE as usize * 15 / 100;
    let in_pause: Vec<usize> = bounds
        .windows(2)
        .map(|w| (w[0].1 + w[1].0) / 2 - len / 2)
        .collect();
    let (oa, ob) = *sp_active
        .iter()
        .find(|&&(a, b)| b - a > len * 2 && a > bounds[1].0)
        .unwrap();
    let over = oa + (ob - oa - len) / 2;
    for &p in in_pause.iter().chain(std::iter::once(&over)) {
        for (j, v) in knock(len, 0.3).iter().enumerate() {
            x[p + j] += v;
        }
    }
    let input = write(&dir, "in.wav", &x);

    let mut on = isolated();
    on.knock.reduction_db = 15.0;
    let mut off = on.clone();
    off.knock.enabled = false;
    let r_on = run(&input, &dir.join("on.wav"), &on);
    run(&input, &dir.join("off.wav"), &off);
    let y_on = read_wav(&dir.join("on.wav")).1.remove(0);
    let y_off = read_wav(&dir.join("off.wav")).1.remove(0);
    let k = r_on.knock.unwrap();
    eprintln!("Knackser-Report: {k:?}");
    assert!(k.regions >= in_pause.len(), "{k:?}");

    // Knackser in Pausen: um ≥ 12 dB leiser.
    for &p in &in_pause {
        let d = rms_db(&y_on, p, p + len) - rms_db(&y_off, p, p + len);
        eprintln!("Knackser bei {:.2} s: {d:+.1} dB", p as f64 / RATE as f64);
        assert!(d <= -12.0, "Knackser bei {p}: nur {d:+.1} dB");
    }
    // Knackser über Sprache: Sprachanteil über 300 Hz ändert sich < 0,5 dB.
    let hp_on = highpassed(&y_on, 400.0);
    let hp_off = highpassed(&y_off, 400.0);
    let d = rms_db(&hp_on, over, over + len) - rms_db(&hp_off, over, over + len);
    eprintln!("Sprache über 400 Hz beim Knackser in der Silbe: {d:+.2} dB");
    assert!(d.abs() < 0.5, "{d:+.2} dB");
    // Sprache fern von Knacksern: unverändert (< 0,05 dB).
    let far: Vec<(usize, usize)> = sp_active
        .iter()
        .copied()
        .filter(|&(a, b)| {
            in_pause
                .iter()
                .chain(std::iter::once(&over))
                .all(|&p| b + RATE as usize / 2 < p || a > p + len + RATE as usize / 2)
        })
        .collect();
    let d = ranges_db(&y_on, &far) - ranges_db(&y_off, &far);
    assert!(d.abs() < 0.05, "Sprache fern von Knacksern: {d:+.3} dB");
}

/// Sprache bei −24 dBFS; in drei Silben 300 ms um 12 dB lauter.
fn speech_with_outliers(dir: &Path) -> (PathBuf, Vec<f32>, Ranges, Ranges) {
    let sp = synth_speech(RATE, 20.0, 140.0, -24.0, 5);
    let mut x = sp.samples.clone();
    let w = RATE as usize * 3 / 10;
    let mut bursts = Vec::new();
    for &(a, b) in sp.active.iter().filter(|(a, b)| b - a > w) {
        if bursts.len() == 3 {
            break;
        }
        if bursts
            .last()
            .is_some_and(|&(_, e)| a < e + RATE as usize * 2)
        {
            continue;
        }
        let s = a + (b - a - w) / 2;
        x[s..s + w].iter_mut().for_each(|v| *v *= 4.0); // +12 dB
        bursts.push((s, s + w));
    }
    assert_eq!(bursts.len(), 3);
    let normal: Vec<(usize, usize)> = sp
        .active
        .iter()
        .copied()
        .filter(|&(a, b)| {
            bursts
                .iter()
                .all(|&(s, e)| b + RATE as usize / 2 < s || a > e + RATE as usize / 2)
        })
        .collect();
    (write(dir, "in.wav", &x), x, bursts, normal)
}

#[test]
fn compressor_tames_outliers_and_leaves_speech() {
    let dir = temp_dir("p5-comp");
    let (input, x, bursts, normal) = speech_with_outliers(&dir);
    let before = ranges_db(&x, &bursts) - ranges_db(&x, &normal);
    assert!((before - 12.0).abs() < 1.0, "{before:.2}");

    let mut results = Vec::new();
    for amount in [1.0, 0.4] {
        let mut s = isolated();
        s.denoise.enabled = false;
        s.knock.enabled = false;
        s.compressor.enabled = true;
        s.compressor.amount = amount;
        let out = dir.join(format!("c{amount}.wav"));
        let r = run(&input, &out, &s);
        let y = read_wav(&out).1.remove(0);
        let excess = ranges_db(&y, &bursts) - ranges_db(&y, &normal);
        let speech_change = ranges_db(&y, &normal) - ranges_db(&x, &normal);
        eprintln!(
            "Stärke {amount}: Ausreißer {excess:+.1} dB über Sprache (vorher {before:+.1}), \
             Sprache {speech_change:+.2} dB, {:?}",
            r.compressor
        );
        // Default-Stärke 40 %: Sprache praktisch unberührt; bei 100 % (6:1)
        // werden auch Silbenspitzen etwas erfasst.
        let limit = if amount <= 0.4 { 0.5 } else { 1.5 };
        assert!(
            speech_change.abs() < limit,
            "Sprache {speech_change:+.2} dB"
        );
        results.push(excess);
    }
    // 100 %: Ausreißer um ≥ 4 dB gezähmt; 40 %: spürbar, aber schwächer.
    assert!(results[0] <= before - 4.0, "100 %: {:.1}", results[0]);
    assert!(
        results[1] < before - 1.0 && results[1] > results[0],
        "40 %: {:.1}",
        results[1]
    );

    // Stärke 0 ⇒ bitidentisch zu „aus“.
    let mut zero = isolated();
    zero.denoise.enabled = false;
    zero.knock.enabled = false;
    zero.compressor.enabled = true;
    zero.compressor.amount = 0.0;
    let mut off = zero.clone();
    off.compressor.enabled = false;
    run(&input, &dir.join("z.wav"), &zero);
    run(&input, &dir.join("o.wav"), &off);
    assert_eq!(
        std::fs::read(dir.join("z.wav")).unwrap(),
        std::fs::read(dir.join("o.wav")).unwrap()
    );
}

struct GateScene {
    path: PathBuf,
    /// (Pausenbeginn, Pausenende) in Samples: letzte bzw. erste aktive Stelle.
    long_pauses: Vec<(usize, usize)>,
    short_pause: (usize, usize),
    phrase_starts: Vec<usize>,
    speech: Vec<(usize, usize)>,
}

fn gate_scene(dir: &Path) -> GateScene {
    let (mut x, speech, bounds) = blocks(140.0, -22.0, 4.0, &[1.2, 0.25, 3.0], 40);
    let noise = pink_noise(RATE, 1, -45.0, x.len() as f64 / RATE as f64 + 0.1, 8);
    for (v, n) in x.iter_mut().zip(&noise[0]) {
        *v += n;
    }
    let pause = |i: usize| (bounds[i].1, bounds[i + 1].0);
    GateScene {
        path: write(dir, "in.wav", &x),
        long_pauses: vec![pause(0), pause(2)],
        short_pause: pause(1),
        phrase_starts: bounds.iter().map(|b| b.0).collect(),
        speech,
    }
}

#[test]
fn pause_floor_reaches_target_and_spares_speech() {
    let dir = temp_dir("p5-gate");
    let sc = gate_scene(&dir);
    let mut on = Settings::default();
    on.knock.enabled = false;
    on.output.bit_depth = BitDepth::F32;
    on.gate.enabled = true;
    on.gate.floor_dbfs = -60.0;
    let mut off = on.clone();
    off.gate.enabled = false;
    let r = run(&sc.path, &dir.join("on.wav"), &on);
    run(&sc.path, &dir.join("off.wav"), &off);
    let y_on = read_wav(&dir.join("on.wav")).1.remove(0);
    let y_off = read_wav(&dir.join("off.wav")).1.remove(0);
    eprintln!("Gate-Report: {:?}", r.gate);

    // Inneres der langen Pausen (≥ 0,4 s Abstand zur Sprache): −60 ± 2 dBFS.
    let m = RATE as usize * 4 / 10;
    for &(a, b) in &sc.long_pauses {
        let l = rms_db(&y_on, a + m, b - m);
        eprintln!(
            "Pause {:.2}–{:.2} s: {l:.1} dBFS (ohne: {:.1})",
            a as f64 / RATE as f64,
            b as f64 / RATE as f64,
            rms_db(&y_off, a + m, b - m)
        );
        assert!((l + 60.0).abs() <= 2.0, "Pausenpegel {l:.1} dBFS");
    }
    // Sprache: < 0,5 dB Änderung.
    let d = ranges_db(&y_on, &sc.speech) - ranges_db(&y_off, &sc.speech);
    assert!(d.abs() < 0.5, "Sprache {d:+.2} dB");
    // Phrasenanfänge: erste 20 ms höchstens 1 dB leiser.
    let w = RATE as usize / 50;
    for &s in &sc.phrase_starts {
        let d = rms_db(&y_on, s, s + w) - rms_db(&y_off, s, s + w);
        assert!(
            d >= -1.0,
            "Phrasenanfang bei {:.2} s: {d:+.2} dB",
            s as f64 / RATE as f64
        );
    }
    // Kurze Pause (< 400 ms): unverändert.
    let (a, b) = sc.short_pause;
    let d = rms_db(&y_on, a, b) - rms_db(&y_off, a, b);
    assert!(d.abs() < 0.1, "kurze Pause {d:+.2} dB");
    // Keine Sprünge: Gain-Verhältnis ändert sich je Millisekunde um < 1 dB.
    let ms = RATE as usize / 1000;
    let gains: Vec<f64> = (0..y_on.len() / ms)
        .map(|k| rms_db(&y_on, k * ms, (k + 1) * ms) - rms_db(&y_off, k * ms, (k + 1) * ms))
        .collect();
    let jump = gains
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0f64, f64::max);
    assert!(jump < 1.0, "größter Gain-Sprung {jump:.2} dB/ms");

    // Pausen-Absenkung mit Tiefe 0 ⇒ bitidentisch zu „aus“.
    let mut zero = on.clone();
    zero.gate.max_attenuation_db = 0.0;
    run(&sc.path, &dir.join("zero.wav"), &zero);
    assert_eq!(
        std::fs::read(dir.join("zero.wav")).unwrap(),
        std::fs::read(dir.join("off.wav")).unwrap()
    );
}
