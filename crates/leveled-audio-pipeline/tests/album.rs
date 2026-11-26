//! Akzeptanztests Phase 4: Album-Modus.

use leveled_audio_io::SampleFormat;
use leveled_audio_pipeline::{
    CancelToken, JobOptions, LoudnessMode, NoProgress, Settings, process_album,
};
use leveled_audio_testkit::{pink_noise, synth_speech, temp_dir, write_wav};
use std::path::PathBuf;

const RATE: u32 = 48_000;

/// Drei Sprachdateien mit deutlich verschiedener Lautheit, eine davon kaputt
/// auf Wunsch.
fn inputs(dir: &std::path::Path, broken: bool) -> Vec<(PathBuf, PathBuf)> {
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut items = Vec::new();
    for (k, level) in [-20.0, -26.0, -31.0].into_iter().enumerate() {
        let sp = synth_speech(RATE, 12.0, 120.0 + 40.0 * k as f64, level, 7 + k as u64);
        let noise = pink_noise(RATE, 1, -65.0, 12.1, 30 + k as u64);
        let x: Vec<f32> = sp
            .samples
            .iter()
            .zip(&noise[0])
            .map(|(s, n)| s + n)
            .collect();
        let p = dir.join(format!("track{k}.wav"));
        write_wav(&p, RATE, SampleFormat::Int24, &vec![x]);
        items.push((p, out_dir.join(format!("track{k}.wav"))));
    }
    if broken {
        let p = dir.join("kaputt.wav");
        std::fs::write(&p, b"RIFF\x10\x00\x00\x00WAVEjunk").unwrap();
        items.insert(1, (p, out_dir.join("kaputt.wav")));
    }
    items
}

fn album_settings() -> Settings {
    let mut s = Settings::default();
    s.loudness.mode = LoudnessMode::Album;
    s
}

#[test]
fn album_shares_one_gain_and_hits_target() {
    let dir = temp_dir("album-gain");
    let items = inputs(&dir, false);
    let s = album_settings();
    let r = process_album(
        items.clone(),
        &s,
        &JobOptions::default(),
        2,
        &NoProgress,
        &CancelToken::new(),
    );
    let album = &r.album;
    eprintln!("{album:?}");
    let reps: Vec<_> = r.files.into_iter().map(|f| f.unwrap()).collect();
    let gains: Vec<f64> = reps
        .iter()
        .map(|f| f.normalization_gain_db.unwrap())
        .collect();
    for g in &gains {
        assert_eq!(*g, album.gain_db.unwrap(), "gleicher Gain: {gains:?}");
    }
    assert_eq!(album.files, 3);
    let out = album.loudness_out_lufs.unwrap();
    assert!((out + 16.0).abs() <= 0.5, "Album-Lautheit {out:.2} LUFS");

    // Relative Unterschiede bleiben grob erhalten (Per-File brächte alle auf
    // −16). Der Limiter staucht die lauteste Datei (+11,7 dB Gain) etwas.
    let outs: Vec<f64> = reps
        .iter()
        .map(|f| f.loudness_out.unwrap().integrated_lufs.unwrap())
        .collect();
    let ins: Vec<f64> = reps
        .iter()
        .map(|f| f.loudness_in.integrated_lufs.unwrap())
        .collect();
    eprintln!("ein {ins:.2?}  aus {outs:.2?}");
    assert!(outs[0] > outs[1] && outs[1] > outs[2], "{outs:?}");
    assert!(outs[0] - outs[2] > 6.0, "Abstand bleibt: {outs:?}");
    for (_, o) in &items {
        assert!(o.exists());
    }
}

#[test]
fn album_ignores_broken_file() {
    let dir = temp_dir("album-broken");
    let items = inputs(&dir, true);
    let mut s = album_settings();
    // Ohne Limiter ist das Rendern linear: die Album-Lautheit trifft exakt.
    // Ziel so, dass die lauteste Datei nicht übersteuert.
    s.limiter.enabled = false;
    s.loudness.target_lufs = -24.0;
    let r = process_album(
        items.clone(),
        &s,
        &JobOptions::default(),
        3,
        &NoProgress,
        &CancelToken::new(),
    );
    assert!(r.files[1].is_err(), "kaputte Datei meldet Fehler");
    assert_eq!(r.album.files, 3);
    assert!(
        r.album.warnings.iter().any(|w| w.contains("1 von 4")),
        "{:?}",
        r.album.warnings
    );
    let out = r.album.loudness_out_lufs.unwrap();
    assert!((out + 24.0).abs() <= 0.05, "ohne Limiter exakt: {out:.3}");

    // Alle verarbeiteten Dateien mit demselben Gain.
    let g: Vec<f64> = [0, 2, 3]
        .iter()
        .map(|&i| r.files[i].as_ref().unwrap().normalization_gain_db.unwrap())
        .collect();
    assert!(g.iter().all(|&x| x == g[0]), "{g:?}");
    assert!(!items[1].1.exists(), "keine Ausgabe für die kaputte Datei");
}
