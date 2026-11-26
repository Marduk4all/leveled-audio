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

/// Prüft einen JSON-Report gegen `docs/report.schema.json`.
fn validate_report(path: &Path) -> serde_json::Value {
    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/report.schema.json");
    let schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(schema_path).unwrap()).unwrap();
    let validator = jsonschema::draft202012::new(&schema).expect("Schema gültig");
    let rep: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&rep)
        .map(|e| format!("{} bei {}", e, e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "Schema-Fehler:\n{}", errors.join("\n"));
    rep
}

#[test]
fn json_reports_validate_against_schema() {
    let dir = setup("cli-schema");
    std::fs::write(dir.join("in/kaputt.wav"), b"RIFF....WAVEjunk").unwrap();
    let out = dir.join("out");
    // Einmal vorab, damit beim zweiten Lauf eine Datei übersprungen wird.
    leveled_audio(&["process", s(&dir.join("in/a.wav")), "-o", s(&out), "-q"]);
    let input = dir.join("in");

    for (k, extra) in [
        vec!["--skip-existing", "--export-delta"],
        vec!["--overwrite", "--loudness-mode", "album", "--no-denoise"],
        vec![
            "--overwrite",
            "--no-leveler",
            "--no-limiter",
            "--bit-depth",
            "32f",
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let json = dir.join(format!("report{k}.json"));
        let mut args = vec![
            "process",
            s(&input),
            "-r",
            "-o",
            s(&out),
            "--report-json",
            s(&json),
            "-q",
        ];
        args.extend(extra);
        let o = leveled_audio(&args);
        assert_eq!(
            o.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let rep = validate_report(&json);
        // Reihenfolge deterministisch (sortierte Eingaben), unabhängig von der Rechenzeit.
        let names: Vec<&str> = rep["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["input"].as_str().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert_eq!(rep["summary"]["errors"], 1);
        if k == 0 {
            assert_eq!(rep["summary"]["skipped"], 1);
        }
        if k == 1 {
            assert_eq!(rep["album"]["files"], 2);
        }
    }

    let json = dir.join("analyze.json");
    let o = leveled_audio(&[
        "analyze",
        s(&dir.join("in")),
        "-r",
        "--json",
        s(&json),
        "-q",
    ]);
    assert_eq!(o.status.code(), Some(1));
    let rep = validate_report(&json);
    assert_eq!(rep["command"], "analyze");

    // Gegenprobe: das Schema ist streng (unbekannte Felder, falsche Typen).
    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/report.schema.json");
    let schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(schema_path).unwrap()).unwrap();
    let v = jsonschema::draft202012::new(&schema).unwrap();
    let mut bad = rep.clone();
    bad["files"][0]["report"]["extra"] = 1.into();
    assert!(!v.is_valid(&bad));
    let mut bad = rep.clone();
    bad["files"][0]["report"]["loudness_in"]["integrated_lufs"] = "laut".into();
    assert!(!v.is_valid(&bad));
}

#[test]
fn settings_hash_is_stable_and_tracks_changes() {
    let dir = setup("cli-hash");
    let a = dir.join("in/a.wav");
    let run = |extra: &[&str], name: &str| {
        let json = dir.join(name);
        let mut args = vec!["analyze", s(&a), "--json", s(&json), "-q"];
        args.extend_from_slice(extra);
        assert!(leveled_audio(&args).status.success());
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        v["settings_hash"].as_str().unwrap().to_string()
    };
    let a = run(&[], "a.json");
    assert_eq!(a, run(&[], "b.json"));
    assert_ne!(a, run(&["--set", "denoise.reduction_db=14"], "c.json"));
}

#[test]
fn batch_of_100_files_with_one_broken() {
    let dir = temp_dir("cli-batch100");
    let input = dir.join("in");
    std::fs::create_dir_all(&input).unwrap();
    let clip = speechlike(16_000, 1, -26.0, 0.6, 9);
    for i in 0..100 {
        let p = input.join(format!("f{i:03}.wav"));
        if i == 37 {
            std::fs::write(&p, b"RIFF\x24\x00\x00\x00WAVEfmt ").unwrap();
        } else {
            write_wav(&p, 16_000, SampleFormat::Int16, &clip);
        }
    }
    let out = dir.join("out");
    let json = dir.join("report.json");
    let o = leveled_audio(&[
        "process",
        s(&input),
        "-o",
        s(&out),
        "--report-json",
        s(&json),
        "-j",
        "4",
        "-q",
    ]);
    assert_eq!(o.status.code(), Some(1));
    let rep = validate_report(&json);
    assert_eq!(rep["summary"]["ok"], 99);
    assert_eq!(rep["summary"]["errors"], 1);
    let bad = &rep["files"][37];
    assert_eq!(bad["status"], "error");
    assert!(bad["message"].as_str().unwrap().contains("f037.wav"));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 99);
}

#[test]
fn preset_layering_and_set() {
    let dir = setup("cli-layering");
    let a = dir.join("in/a.wav");
    let out = dir.join("out");

    // Preset-Datei auf Basis ebu-r128, dann --set, dann eigenes Flag.
    let preset = dir.join("funk.toml");
    std::fs::write(
        &preset,
        "schema_version = 1\nname = \"funk\"\nbase = \"ebu-r128\"\n\n[leveler]\nstrength = 0.3\n",
    )
    .unwrap();
    let json = dir.join("r.json");
    let o = leveled_audio(&[
        "process",
        s(&a),
        "-o",
        s(&out),
        "--preset",
        s(&preset),
        "--set",
        "leveler.strength=0.8",
        "--set",
        "limiter.ceiling_dbtp=-2",
        "--ceiling-dbtp",
        "-1.5",
        "--report-json",
        s(&json),
        "-q",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let rep = validate_report(&json);
    let st = &rep["settings"];
    assert_eq!(st["loudness"]["target_lufs"], -23.0, "vom Basis-Preset");
    assert_eq!(st["leveler"]["strength"], 0.8, "--set schlägt Datei");
    assert_eq!(st["limiter"]["ceiling_dbtp"], -1.5, "Flag schlägt --set");

    // `preset show` ergibt eine ladbare Datei.
    let shown = leveled_audio(&["preset", "show", s(&preset)]);
    assert!(shown.status.success());
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(
        text.starts_with("schema_version = 1\nname = \"funk\""),
        "{text}"
    );
    assert!(text.contains("target_lufs = -23.0"));
    let copy = dir.join("kopie.toml");
    std::fs::write(&copy, text.as_bytes()).unwrap();
    assert!(
        leveled_audio(&["preset", "show", s(&copy)])
            .status
            .success()
    );

    // Fehler: unbekannter Schlüssel in Datei und in --set, ungültiger Wert.
    std::fs::write(dir.join("tippfehler.toml"), "[leveler]\nstrenght = 0.3\n").unwrap();
    for args in [
        vec!["--preset", s(&dir.join("tippfehler.toml"))],
        vec!["--set", "leveler.strenght=0.3"],
        vec!["--set", "leveler.strength=3"],
        vec!["--set", "loudness.mode=radio"],
    ] {
        let mut all = vec!["process", s(&a), "-o", s(&out), "--overwrite"];
        all.extend(args.iter());
        let o = leveled_audio(&all);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.starts_with("Fehler:"), "{err}");
    }
}
