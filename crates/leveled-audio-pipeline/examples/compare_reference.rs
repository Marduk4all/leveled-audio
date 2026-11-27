//! Vergleich einer Ausgabe von Leveled Audio mit einer Referenzfassung (z. B. von einem anderen Werkzeug bearbeitet).
//!
//! Aufruf:
//! `cargo run --release -p leveled-audio-pipeline --example compare_reference -- <unsere.wav> <referenz.wav> [--table]`
//!
//! Gibt aus: integrierte Lautheit, True Peak, LRA beider Dateien; Pausenpegel
//! (leiseste 5 % der 50-ms-Abschnitte); Anteil < 150 Hz und > 6 kHz; Abweichung
//! des 250-ms-Pegelverlaufs in Sprachbereichen (nach Angleich der Gesamtlautheit).
//! Mit `--table` zusätzlich den Pegelverlauf beider Dateien.

use leveled_audio_analysis::LoudnessMeter;
use std::f64::consts::PI;

fn read(path: &str) -> (Vec<Vec<f32>>, u32) {
    let mut src = leveled_audio_io::open_source(std::path::Path::new(path)).expect("Datei öffnen");
    let spec = src.spec();
    let ch = spec.channels as usize;
    let mut all = vec![Vec::new(); ch];
    let mut buf = vec![Vec::new(); ch];
    loop {
        let n = src.read(&mut buf, 65_536).expect("lesen");
        if n == 0 {
            break;
        }
        for c in 0..ch {
            all[c].extend_from_slice(&buf[c][..n]);
        }
    }
    (all, spec.sample_rate)
}

fn mono(x: &[Vec<f32>]) -> Vec<f64> {
    let ch = x.len() as f64;
    (0..x[0].len())
        .map(|i| x.iter().map(|c| c[i] as f64).sum::<f64>() / ch)
        .collect()
}

/// Leistung je Fenster in dB.
fn levels(x: &[f64], w: usize) -> Vec<f64> {
    x.chunks_exact(w)
        .map(|c| 10.0 * (c.iter().map(|v| v * v).sum::<f64>() / w as f64 + 1e-20).log10())
        .collect()
}

/// Ein-Pol-Tiefpass; liefert Energieanteile unter `lo` und über `hi` (dB).
fn band_shares(x: &[f64], rate: u32, lo: f64, hi: f64) -> (f64, f64) {
    let a = (-2.0 * PI * lo / rate as f64).exp();
    let b = (-2.0 * PI * hi / rate as f64).exp();
    let (mut l, mut h, mut lsum, mut hsum, mut tot) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for &v in x {
        l = (1.0 - a) * v + a * l;
        h = (1.0 - b) * v + b * h;
        lsum += l * l;
        hsum += (v - h) * (v - h);
        tot += v * v;
    }
    (10.0 * (lsum / tot).log10(), 10.0 * (hsum / tot).log10())
}

fn quantile(v: &[f64], q: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * q) as usize]
}

struct Summary {
    lufs: f64,
    tp: f64,
    lra: f64,
    pause: f64,
    low: f64,
    high: f64,
    lv: Vec<f64>,
}

fn summarize(path: &str) -> Summary {
    let (x, rate) = read(path);
    let mut m = LoudnessMeter::new(x.len(), rate).unwrap();
    m.add(&x, x[0].len()).unwrap();
    let st = m.stats().unwrap();
    let mo = mono(&x);
    let fine = levels(&mo, rate as usize / 20);
    let (low, high) = band_shares(&mo, rate, 150.0, 6000.0);
    Summary {
        lufs: st.integrated_lufs.unwrap_or(f64::NAN),
        tp: st.true_peak_dbtp.unwrap_or(f64::NAN),
        lra: st.lra_lu.unwrap_or(f64::NAN),
        pause: quantile(&fine, 0.05),
        low,
        high,
        lv: levels(&mo, rate as usize / 4),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("Aufruf: compare_reference <unsere.wav> <referenz.wav> [--table]");
        std::process::exit(2);
    }
    let ours = summarize(&args[0]);
    let refs = summarize(&args[1]);
    println!("                 leveled-audio   Referenz");
    println!("Lautheit LUFS   {:9.1}  {:9.1}", ours.lufs, refs.lufs);
    println!("True Peak dBTP  {:9.1}  {:9.1}", ours.tp, refs.tp);
    println!("LRA LU          {:9.1}  {:9.1}", ours.lra, refs.lra);
    println!(
        "Pausen dBFS     {:9.1}  {:9.1}   (leiseste 5 % der 50-ms-Abschnitte)",
        ours.pause, refs.pause
    );
    println!("Anteil <150 Hz  {:9.1}  {:9.1}   dB", ours.low, refs.low);
    println!("Anteil >6 kHz   {:9.1}  {:9.1}   dB", ours.high, refs.high);

    // Sprachbereiche: 250-ms-Fenster, in denen die Referenz höchstens 15 dB
    // unter ihrem Median liegt. Pegel vorher um die Lautheitsdifferenz angleichen.
    let n = ours.lv.len().min(refs.lv.len());
    let off = ours.lufs - refs.lufs;
    let med = quantile(&refs.lv[..n], 0.5);
    let diffs: Vec<f64> = (0..n)
        .filter(|&k| refs.lv[k] > med - 15.0)
        .map(|k| ours.lv[k] - off - refs.lv[k])
        .collect();
    if !diffs.is_empty() {
        let abs: Vec<f64> = diffs.iter().map(|d| d.abs()).collect();
        println!(
            "Sprache: Abweichung je 250 ms (angeglichen) Median {:+.1} dB, |Median| {:.1} dB, 90 % ≤ {:.1} dB ({} Fenster)",
            quantile(&diffs, 0.5),
            quantile(&abs, 0.5),
            quantile(&abs, 0.9),
            diffs.len()
        );
    }
    if args.iter().any(|a| a == "--table") {
        println!("\n  Zeit  leveled-audio  Referenz");
        for k in 0..n {
            println!(
                "{:6.2} {:9.1} {:9.1}",
                k as f64 * 0.25,
                ours.lv[k],
                refs.lv[k]
            );
        }
    }
}
