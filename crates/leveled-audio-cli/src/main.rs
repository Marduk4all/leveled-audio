//! leveled-audio – Batch-Verarbeitung von Sprachaufnahmen (WAV).

mod args;
mod inputs;
mod summary;
mod ui;

use anyhow::{Context, Result, bail};
use args::{Cli, Command, PresetCmd, ProcessArgs, ProfileCmd};
use clap::Parser;
use inputs::{Job, collect_inputs, plan_outputs};
use leveled_audio_pipeline::{
    CancelToken, JobOptions, NoProgress, PRESET_NAMES, ProgressSink, Settings, analyze_file,
    learn_profile, load_profile, process_file, run_batch, save_profile,
};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use summary::{FileOutcome, print_summary, write_json};

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
        Command::Process(a) => cmd_process(a, cli.quiet),
        Command::Analyze(a) => {
            let settings = load_settings(&a.preset)?;
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
            finish(&results, &cancel, a.json.as_deref(), None, cli.quiet)
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
                        let s = Settings::preset(name).expect("Preset");
                        println!("{name:<16} {:>6.1} LUFS", s.loudness.target_lufs);
                    }
                }
                PresetCmd::Show { name } => {
                    let s = load_settings(&name)?;
                    print!("{}", toml::to_string_pretty(&s)?);
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
        let s: Settings = toml::from_str(&text)
            .with_context(|| format!("Preset-Datei {} fehlerhaft", path.display()))?;
        return Ok(s);
    }
    bail!(
        "unbekanntes Preset „{preset}“ (eingebaut: {}; oder Pfad zu einer .toml-Datei)",
        PRESET_NAMES.join(", ")
    )
}

fn cmd_process(a: ProcessArgs, quiet: bool) -> Result<u8> {
    let mut settings = load_settings(&a.preset)?;
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
    if let Some(p) = a.leveler_strength {
        if !(0.0..=100.0).contains(&p) {
            bail!("--leveler-strength muss zwischen 0 und 100 liegen");
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
    let results = run_batch(plan, jobs, |id, job| {
        let outcome = match job {
            Job::Skip { input, output } => FileOutcome::skipped(input.display, input.path, output),
            Job::Process { input, output } => {
                let r = process_file(
                    &input.path,
                    &output,
                    &settings,
                    &opts,
                    id,
                    &ui as &dyn ProgressSink,
                    &cancel,
                );
                FileOutcome::from_result(input.display, input.path, r)
            }
        };
        ui.file_done(id);
        outcome
    });
    ui.finish();
    finish(
        &results,
        &cancel,
        a.report_json.as_deref(),
        Some(&settings),
        quiet,
    )
}

fn finish(
    results: &[FileOutcome],
    cancel: &CancelToken,
    json: Option<&std::path::Path>,
    settings: Option<&Settings>,
    quiet: bool,
) -> Result<u8> {
    if !quiet {
        print_summary(results);
    }
    if let Some(path) = json {
        write_json(path, results, settings)?;
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
