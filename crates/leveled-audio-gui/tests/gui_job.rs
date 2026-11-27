//! Tests der GUI-Logik ohne Oberfläche (PLAN Phase 6).

use leveled_audio_cli::summary::{Status, annotate_preset};
use leveled_audio_gui::job::{self, Existing, Request};
use leveled_audio_io::SampleFormat;
use leveled_audio_pipeline::{
    CancelToken, JobOptions, NoProgress, PARAMS, ParamKind, Settings, process_file,
};
use leveled_audio_testkit::{speechlike, temp_dir, write_wav};
use std::path::Path;

/// Jeden Parameter auf einen anderen gültigen Wert setzen, wie über die Regler.
fn edited() -> Settings {
    let mut s = Settings::default();
    for p in PARAMS {
        let v: toml::Value = match p.kind {
            ParamKind::Bool => (!s.get_value(p.key).unwrap().as_bool().unwrap()).into(),
            ParamKind::Float { min, max, .. } => (min + (max - min) * 0.37).into(),
            ParamKind::Choice(c) => c[c.len() - 1].0.into(),
            ParamKind::IntChoice(c) => c[0].into(),
            ParamKind::FftSize => 1024.into(),
        };
        // Profil-Dauern zusammen gültig halten.
        s.set_value(p.key, v).unwrap();
    }
    s.denoise.profile.min_duration_s = 0.5;
    s.denoise.profile.target_duration_s = 2.0;
    // Für den Vergleich alles einschalten, was eine Wirkung hat.
    for k in [
        "highpass.enabled",
        "denoise.enabled",
        "knock.enabled",
        "leveler.enabled",
        "gate.enabled",
        "compressor.enabled",
        "loudness.enabled",
        "limiter.enabled",
    ] {
        s.set_value(k, true.into()).unwrap();
    }
    s.loudness.mode = leveled_audio_pipeline::LoudnessMode::PerFile;
    s.validate().unwrap();
    s
}

#[test]
fn every_parameter_is_settable_and_preset_roundtrips() {
    let s = edited();
    assert_ne!(s, Settings::default());
    // „Preset speichern…“ in der GUI = annotiertes TOML; die Kommandozeile
    // liest es mit `--preset` über `from_preset_toml`.
    let text = annotate_preset(&s.to_preset_toml("gui-test"));
    let back = Settings::from_preset_toml(&text).unwrap();
    assert_eq!(back, s);
    back.validate().unwrap();
}

fn input(dir: &Path) -> std::path::PathBuf {
    let d = dir.join("in");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    write_wav(
        &d.join("a.wav"),
        48_000,
        SampleFormat::Int24,
        &speechlike(48_000, 2, -28.0, 6.0, 4),
    );
    write_wav(
        &d.join("sub/b.wav"),
        44_100,
        SampleFormat::Int16,
        &speechlike(44_100, 1, -22.0, 5.0, 5),
    );
    std::fs::write(d.join("notiz.txt"), "kein Audio").unwrap();
    d
}

fn request(inputs: Vec<std::path::PathBuf>, s: Settings, label: &str) -> Request {
    Request {
        inputs,
        recursive: true,
        settings: s,
        preset_label: label.into(),
        jobs: 2,
    }
}

/// Exportierte Datei neben der Eingabe.
fn target(input: &Path, label: &str) -> std::path::PathBuf {
    job::output_path(input, label)
}

#[test]
fn gui_job_matches_direct_pipeline_and_saved_preset() {
    let dir = temp_dir("gui-job");
    let d = input(&dir);
    let s = edited();

    // GUI-Lauf: Ausgabe neben den Originalen.
    let req = request(vec![d.clone()], s.clone(), "gui");
    assert_eq!(job::count_inputs(&req.inputs, true).unwrap(), 2);
    let plan = job::plan(&req, Existing::Error).unwrap();
    let out = job::execute(&req, plan, &NoProgress, &CancelToken::new(), &|_| {});
    assert!(
        out.files
            .iter()
            .all(|f| matches!(f.status, Status::Ok { .. }))
    );

    // Dieselbe Datei direkt über die Pipeline: byte-identisch.
    let direct = dir.join("direct.wav");
    process_file(
        &d.join("a.wav"),
        &direct,
        &s,
        &JobOptions::default(),
        0,
        &NoProgress,
        &CancelToken::new(),
    )
    .unwrap();
    let a_gui = d.join("a-leveled-gui.wav");
    assert_eq!(target(&d.join("a.wav"), "gui"), a_gui);
    assert_eq!(
        std::fs::read(&a_gui).unwrap(),
        std::fs::read(&direct).unwrap()
    );
    assert!(d.join("sub/b-leveled-gui.wav").exists());
    // Exportierte Dateien zählen beim nächsten Ordner-Import nicht mit.
    assert_eq!(
        job::count_inputs(std::slice::from_ref(&d), true).unwrap(),
        2
    );
    // Einzeln hinzugefügt schon.
    assert_eq!(
        job::count_inputs(std::slice::from_ref(&a_gui), true).unwrap(),
        1
    );

    // Gespeichertes Preset wieder geladen (wie `leveled-audio --preset`): gleiche Dateien.
    let loaded =
        Settings::from_preset_toml(&annotate_preset(&s.to_preset_toml("gui-test"))).unwrap();
    let req2 = request(vec![d.clone()], loaded, "preset");
    let plan2 = job::plan(&req2, Existing::Error).unwrap();
    job::execute(&req2, plan2, &NoProgress, &CancelToken::new(), &|_| {});
    for f in ["a.wav", "sub/b.wav"] {
        let f = d.join(f);
        assert_eq!(
            std::fs::read(target(&f, "gui")).unwrap(),
            std::fs::read(target(&f, "preset")).unwrap(),
            "{}",
            f.display()
        );
    }
}

#[test]
fn import_dedupes_and_ignores_non_wav_and_reports_broken_files() {
    let dir = temp_dir("gui-import");
    let d = input(&dir);
    std::fs::write(d.join("kaputt.wav"), b"RIFF....WAVEjunk").unwrap();
    // Ordner und eine Datei daraus doppelt: jede Datei nur einmal.
    let inputs = vec![d.clone(), d.join("a.wav"), d.join("a.wav")];
    assert_eq!(job::count_inputs(&inputs, true).unwrap(), 3);
    assert_eq!(job::count_inputs(&inputs, false).unwrap(), 2);

    let req = request(inputs, Settings::default(), "out");
    assert!(job::existing_outputs(&req).unwrap().is_empty());
    let plan = job::plan(&req, Existing::Error).unwrap();
    let out = job::execute(&req, plan, &NoProgress, &CancelToken::new(), &|_| {});
    let errors = out.files.iter().filter(|f| f.is_error()).count();
    let ok = out
        .files
        .iter()
        .filter(|f| matches!(f.status, Status::Ok { .. }))
        .count();
    assert_eq!((ok, errors), (2, 1));

    // Ziel existiert: Rückfrage nötig; ohne Antwort Abbruch vor dem Start,
    // „überspringen“ und „überschreiben“ gehen.
    let again = request(vec![d.clone()], Settings::default(), "out");
    assert_eq!(job::existing_outputs(&again).unwrap().len(), 2);
    assert!(job::plan(&again, Existing::Error).is_err());
    let skip = job::plan(&again, Existing::Skip).unwrap();
    assert!(
        skip.jobs
            .iter()
            .all(|j| matches!(j, leveled_audio_cli::inputs::Job::Skip { .. })
                || j.display().contains("kaputt"))
    );
    assert!(job::plan(&again, Existing::Overwrite).is_ok());
}

#[test]
fn cancel_leaves_no_partial_files() {
    let dir = temp_dir("gui-cancel");
    let d = input(&dir);
    let before = walk(&d);
    let req = request(vec![d.clone()], Settings::default(), "out");
    let plan = job::plan(&req, Existing::Error).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let out = job::execute(&req, plan, &NoProgress, &cancel, &|_| {});
    assert!(
        out.files
            .iter()
            .all(|f| matches!(f.status, Status::Cancelled))
    );
    // Neben den Originalen ist nichts dazugekommen.
    assert_eq!(walk(&d), before);
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}
