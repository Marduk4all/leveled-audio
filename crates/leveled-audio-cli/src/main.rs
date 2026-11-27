//! Leveled Audio (Kommandozeile `leveled-audio`) – Batch-Verarbeitung von Sprachaufnahmen (WAV).

mod args;
mod ui;

use anyhow::{Context, Result, bail};
use args::{Cli, Command, PresetCmd, ProcessArgs, ProfileCmd};
use clap::Parser;
use leveled_audio_cli::batch::run_plan;
use leveled_audio_cli::inputs::{collect_inputs, plan_outputs};
use leveled_audio_cli::summary::{
    self, FileOutcome, JsonMeta, print_album, print_summary, write_json,
};
use leveled_audio_pipeline::{
    CancelToken, JobOptions, NoProgress, PRESET_NAMES, ProgressSink, Settings, analyze_file,
    learn_profile, load_profile, preset_description, run_batch, save_profile,
};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const EXIT_FAILURES: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_CANCELLED: u8 = 130;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("Fehler: {e:#}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn install_ctrlc() -> CancelToken {
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let presses = Arc::new(AtomicUsize::new(0));
    let _ = ctrlc::set_handler(move || {
        if presses.fetch_add(1, Ordering::SeqCst) == 0 {
            eprintln!(
                "\nAbbruch angefordert – laufende Dateien werden beendet und Teildateien gelöscht \
                 (nochmals Ctrl+C für sofortiges Ende)."
            );
            c.cancel();
        } else {
            std::process::exit(EXIT_CANCELLED as i32);
        }
    });
    cancel
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(4)
}

fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Command::Process(a) => cmd_process(*a, cli.quiet),
        Command::Analyze(a) => {
            let mut settings = load_settings(&a.preset)?;
            apply_overrides(&mut settings, &a.overrides)?;
            let files = collect_inputs(&a.inputs, a.recursive)?;
            let cancel = install_ctrlc();
            let jobs = a.jobs.unwrap_or_else(default_jobs);
            let names: Vec<String> = files.iter().map(|f| f.display.clone()).collect();
            let ui = ui::Progress::new(&names, cli.quiet);
            let results = run_batch(files, jobs, |id, f| {
                let r = analyze_file(&f.path, &settings, id, &ui as &dyn ProgressSink, &cancel);
                ui.file_done(id);
                FileOutcome::from_result(f.display, f.path, r)
            });
            ui.finish();
            let meta = JsonMeta {
                command: "analyze",
                settings: &settings,
                album: None,
            };
            finish(&results, &cancel, a.json.as_deref(), meta, cli.quiet)
        }
        Command::Profile {
            cmd:
                ProfileCmd::Learn {
                    input,
                    output,
                    preset,
                },
        } => {
            let settings = load_settings(&preset)?;
            let cancel = install_ctrlc();
            let (profile, report) = learn_profile(&input, &settings, &NoProgress, &cancel)?;
            save_profile(&output, &profile)?;
            if !cli.quiet {
                let d = report.denoise.as_ref();
                println!(
                    "Rauschprofil gespeichert: {}\n  Quelle: {:?}, Pegel {:.1} dBFS, Konfidenz {}, {:.1} s aus {} Pause(n)",
                    output.display(),
                    profile.source,
                    profile.level_dbfs(),
                    d.and_then(|d| d.confidence)
                        .map_or("–".into(), |c| format!("{c:.2}")),
                    profile.duration_s,
                    profile.pauses_used,
                );
                for w in &report.warnings {
                    println!("  Hinweis: {w}");
                }
            }
            Ok(0)
        }
        Command::Preset { cmd } => {
            match cmd {
                PresetCmd::List => {
                    for name in PRESET_NAMES {
                        println!("{name:<17} {}", preset_description(name));
                    }
                    println!(
                        "\nEigene Presets: `leveled-audio preset show <name> > mein.toml`, anpassen,\n\
                         dann `--preset mein.toml`. Nicht genannte Werte kommen vom Basis-Preset (`base`)."
                    );
                }
                PresetCmd::Show { name } => {
                    let s = load_settings(&name)?;
                    let label = std::path::Path::new(&name)
                        .file_stem()
                        .map_or(name.clone(), |s| s.to_string_lossy().into_owned());
                    print!("{}", summary::annotate_preset(&s.to_preset_toml(&label)));
                }
            }
            Ok(0)
        }
    }
}

/// Preset-Name oder Pfad zu einer TOML-Datei.
fn load_settings(preset: &str) -> Result<Settings> {
    if let Some(s) = Settings::preset(preset) {
        return Ok(s);
    }
    let path = std::path::Path::new(preset);
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
    {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Preset-Datei {} lesen", path.display()))?;
        let s = Settings::from_preset_toml(&text)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("Preset-Datei {} fehlerhaft", path.display()))?;
        s.validate()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("Preset-Datei {}", path.display()))?;
        return Ok(s);
    }
    bail!(
        "unbekanntes Preset „{preset}“ (eingebaut: {}; oder Pfad zu einer .toml-Datei)",
        PRESET_NAMES.join(", ")
    )
}

/// `--set`-Overrides der Reihe nach anwenden.
fn apply_overrides(settings: &mut Settings, overrides: &[String]) -> Result<()> {
    for o in overrides {
        settings.apply_override(o).map_err(anyhow::Error::msg)?;
    }
    settings.validate().map_err(anyhow::Error::msg)
}

fn cmd_process(a: ProcessArgs, quiet: bool) -> Result<u8> {
    let mut settings = load_settings(&a.preset)?;
    // Reihenfolge: Preset ← --set ← eigene Flags.
    for o in &a.overrides {
        settings.apply_override(o).map_err(anyhow::Error::msg)?;
    }
    if let Some(m) = a.loudness_mode {
        settings.loudness.mode = m.into();
    }
    if let Some(t) = a.target_lufs {
        settings.loudness.target_lufs = t;
    }
    if let Some(c) = a.ceiling_dbtp {
        settings.limiter.ceiling_dbtp = c;
    }
    if let Some(b) = a.bit_depth {
        settings.output.bit_depth = b.into();
    }
    if a.no_limiter {
        settings.limiter.enabled = false;
    }
    if a.no_normalize {
        settings.loudness.enabled = false;
    }
    if a.no_denoise {
        settings.denoise.enabled = false;
    }
    if let Some(r) = a.reduction_db {
        settings.denoise.reduction_db = r;
    }
    if let Some(g) = a.gain_rule {
        settings.denoise.gain_rule = g.into();
    }
    if a.no_leveler {
        settings.leveler.enabled = false;
    }
    if let Some(h) = a.highpass {
        settings.highpass.enabled = true;
        settings.highpass.frequency_hz = h;
    }
    if a.no_highpass {
        settings.highpass.enabled = false;
    }
    if let Some(k) = a.knock_reduction {
        settings.knock.enabled = true;
        settings.knock.reduction_db = k;
    }
    if a.no_knock {
        settings.knock.enabled = false;
    }
    if let Some(p) = a.compression {
        if !(0.0..=100.0).contains(&p) {
            bail!("--compression muss zwischen 0 und 100 liegen");
        }
        settings.compressor.enabled = true;
        settings.compressor.amount = p / 100.0;
    }
    if a.no_compression {
        settings.compressor.enabled = false;
    }
    if let Some(f) = a.pause_floor {
        settings.gate.enabled = true;
        settings.gate.floor_dbfs = f;
    }
    if a.no_pause_floor {
        settings.gate.enabled = false;
    }
    if let Some(p) = a.leveler_strength {
        if !(0.0..=130.0).contains(&p) {
            bail!("--leveler-strength muss zwischen 0 und 130 liegen");
        }
        settings.leveler.strength = p / 100.0;
    }
    settings.validate().map_err(anyhow::Error::msg)?;

    let noise_profile = match &a.noise_profile {
        Some(p) => {
            if !settings.denoise.enabled {
                bail!("--noise-profile und --no-denoise schließen sich aus");
            }
            Some(std::sync::Arc::new(load_profile(p)?))
        }
        None => None,
    };
    if let Some(dir) = &a.save_profiles {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Ordner {} anlegen", dir.display()))?;
    }
    let opts = JobOptions {
        noise_profile,
        save_profile_dir: a.save_profiles.clone(),
        export_delta: a.export_delta,
    };

    let files = collect_inputs(&a.inputs, a.recursive)?;
    let preset_label = std::path::Path::new(&a.preset)
        .file_stem()
        .map_or(a.preset.clone(), |s| s.to_string_lossy().into_owned());
    let plan = plan_outputs(
        files,
        &a.output_dir,
        &a.naming,
        &preset_label,
        settings.loudness.target_lufs,
        a.overwrite,
        a.skip_existing,
    )?;

    let cancel = install_ctrlc();
    let jobs = a.jobs.unwrap_or_else(default_jobs);
    let names: Vec<String> = plan.iter().map(|j| j.display().to_string()).collect();
    let ui = ui::Progress::new(&names, quiet);
    let (results, album) = run_plan(plan, &settings, &opts, jobs, &ui, &cancel, &|id| {
        ui.file_done(id)
    });
    ui.finish();
    if let (false, Some(a)) = (quiet, &album) {
        print_album(a);
    }
    let meta = JsonMeta {
        command: "process",
        settings: &settings,
        album: album.as_ref(),
    };
    finish(&results, &cancel, a.report_json.as_deref(), meta, quiet)
}

fn finish(
    results: &[FileOutcome],
    cancel: &CancelToken,
    json: Option<&std::path::Path>,
    meta: JsonMeta,
    quiet: bool,
) -> Result<u8> {
    if !quiet {
        print_summary(results);
    }
    if let Some(path) = json {
        write_json(path, results, meta)?;
        if !quiet {
            eprintln!("JSON-Report: {}", path.display());
        }
    }
    if cancel.is_cancelled() {
        eprintln!("Abgebrochen.");
        return Ok(EXIT_CANCELLED);
    }
    if results.iter().any(FileOutcome::is_error) {
        return Ok(EXIT_FAILURES);
    }
    Ok(0)
}
