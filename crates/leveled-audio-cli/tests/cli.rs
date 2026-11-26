//! Ende-zu-Ende-Tests des Binaries.

use leveled_audio_io::SampleFormat;
use leveled_audio_testkit::{speechlike, temp_dir, write_wav};
use std::path::Path;
use std::process::{Command, Output};

fn leveled_audio(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_leveled-audio"))
        .args(args)
        .output()
        .expect("leveled-audio starten")
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn setup(name: &str) -> std::path::PathBuf {
    let dir = temp_dir(name);
    let input = dir.join("in");
    std::fs::create_dir_all(input.join("sub")).unwrap();
    write_wav(
        &input.join("a.wav"),
        48_000,
        SampleFormat::Int16,
        &speechlike(48_000, 2, -30.0, 4.0, 1),
    );
    write_wav(
        &input.join("sub/b.WAV"),
        44_100,
        SampleFormat::Int24,
        &speechlike(44_100, 1, -24.0, 4.0, 2),
    );
    std::fs::write(input.join("notes.txt"), "kein Audio").unwrap();
    dir
}

#[test]
fn process_folder_recursive_with_json_report() {
    let dir = setup("cli-process");
    let out = dir.join("out");
    let json = dir.join("report.json");
    let o = leveled_audio(&[
        "process",
        s(&dir.join("in")),
        "-r",
        "-o",
        s(&out),
        "--preset",
        "ebu-r128",
        "--report-json",
        s(&json),
        "-q",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.join("a.wav").exists());
    assert!(out.join("sub/b.WAV").exists());

    let rep: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    let files = rep["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for f in files {
        assert_eq!(f["status"], "ok");
        let i = f["report"]["loudness_out"]["integrated_lufs"]
            .as_f64()
            .unwrap();
        assert!((i + 23.0).abs() < 0.5, "{i}");
    }
    assert_eq!(rep["settings"]["loudness"]["target_lufs"], -23.0);
}

#[test]
fn overwrite_protection() {
    let dir = setup("cli-overwrite");
    let out = dir.join("out");
    let input = dir.join("in/a.wav");
    assert!(
        leveled_audio(&["process", s(&input), "-o", s(&out), "-q"])
            .status
            .success()
    );

    // Ziel existiert → Abbruch vor dem Start, Exit 2.
    let o = leveled_audio(&["process", s(&input), "-o", s(&out), "-q"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("existieren bereits"));

    assert!(
        leveled_audio(&["process", s(&input), "-o", s(&out), "--skip-existing", "-q"])
            .status
            .success()
    );
    assert!(
        leveled_audio(&["process", s(&input), "-o", s(&out), "--overwrite", "-q"])
            .status
            .success()
    );

    // Ausgabe = Eingabe ist immer verboten, auch mit --overwrite.
    let o = leveled_audio(&[
        "process",
        s(&input),
        "-o",
        s(&dir.join("in")),
        "--overwrite",
        "-q",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Eingabedatei selbst"));
}

#[test]
fn broken_file_does_not_stop_batch() {
    let dir = setup("cli-broken");
    std::fs::write(dir.join("in/kaputt.wav"), b"RIFF....WAVEjunk").unwrap();
    let out = dir.join("out");
    let o = leveled_audio(&["process", s(&dir.join("in")), "-o", s(&out)]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out.join("a.wav").exists());
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("FEHLER"), "{stdout}");
    assert!(stdout.contains("1 ok, 1 Fehler"), "{stdout}");
    // Keine Teildateien übrig.
    let leftovers: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".partial"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn analyze_and_presets() {
    let dir = setup("cli-analyze");
    let o = leveled_audio(&["analyze", s(&dir.join("in/a.wav"))]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("a.wav"));

    let o = leveled_audio(&["preset", "show", "podcast-mono"]);
    assert!(o.status.success());
    let toml = String::from_utf8_lossy(&o.stdout);
    assert!(toml.contains("target_lufs = -19.0"), "{toml}");

    // Eigenes Preset aus TOML mit Naming-Platzhaltern.
    let preset = dir.join("mein.toml");
    std::fs::write(
        &preset,
        "[loudness]\ntarget_lufs = -18.0\n[output]\nbit_depth = \"32f\"\n",
    )
    .unwrap();
    let out = dir.join("out");
    let o = leveled_audio(&[
        "process",
        s(&dir.join("in/a.wav")),
        "-o",
        s(&out),
        "--preset",
        s(&preset),
        "--naming",
        "{stem}_{preset}_{lufs}",
        "-q",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.join("a_mein_-18.wav").exists());

    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "[loudness]\ntarget = -18.0\n").unwrap();
    let o = leveled_audio(&[
        "process",
        s(&dir.join("in/a.wav")),
        "-o",
        s(&out),
        "--preset",
        s(&bad),
    ]);
    assert_eq!(o.status.code(), Some(2));
}
