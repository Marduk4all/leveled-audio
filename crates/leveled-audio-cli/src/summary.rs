//! Konsolenzusammenfassung und JSON-Report.

use anyhow::{Context, Result};
use leveled_audio_pipeline::{FileReport, PipelineError, Settings};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Status {
    Ok { report: Box<FileReport> },
    Skipped { output: PathBuf },
    Cancelled,
    Error { message: String },
}

#[derive(Serialize)]
pub struct FileOutcome {
    pub input: PathBuf,
    #[serde(skip)]
    pub display: String,
    #[serde(flatten)]
    pub status: Status,
}

impl FileOutcome {
    pub fn from_result(
        display: String,
        input: PathBuf,
        r: std::result::Result<FileReport, PipelineError>,
    ) -> Self {
        let status = match r {
            Ok(rep) => Status::Ok {
                report: Box::new(rep),
            },
            Err(e) if e.is_cancelled() => Status::Cancelled,
            Err(e) => Status::Error {
                message: e.to_string(),
            },
        };
        FileOutcome {
            input,
            display,
            status,
        }
    }

    pub fn skipped(display: String, input: PathBuf, output: PathBuf) -> Self {
        FileOutcome {
            input,
            display,
            status: Status::Skipped { output },
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.status, Status::Error { .. })
    }
}

fn f(v: Option<f64>) -> String {
    v.map_or("–".into(), |v| format!("{v:.1}"))
}

fn truncate(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n {
        s.to_string()
    } else {
        format!("…{}", c[c.len() - (n - 1)..].iter().collect::<String>())
    }
}

pub fn print_summary(results: &[FileOutcome]) {
    let w = results
        .iter()
        .map(|r| r.display.chars().count())
        .max()
        .unwrap_or(5)
        .clamp(5, 40);
    println!(
        "{:<w$}  {:>6} {:>5} │ {:>6} {:>4} {:>5} │ {:>11} │ {:>6} {:>5} {:>4} │ {:>5} {:>4}  Status",
        "Datei",
        "LUFS",
        "dBTP",
        "dBFS",
        "Konf",
        "NR",
        "Leveler",
        "LUFS",
        "dBTP",
        "LRA",
        "Gain",
        "Lim",
    );
    println!(
        "{:<w$}  {:─^12} │ {:─^17} │ {:─^11} │ {:─^17} │",
        "", " Eingang ", " Rauschen ", "", " Ausgang ",
    );
    let blank = |text: &str| println!("{text:>width$}", width = w + 84);
    let mut notes = Vec::new();
    for r in results {
        let name = truncate(&r.display, w);
        match &r.status {
            Status::Ok { report } => {
                let i = &report.loudness_in;
                let (oi, otp, olra) = report
                    .loudness_out
                    .map_or(("".into(), "".into(), "".into()), |o| {
                        (f(o.integrated_lufs), f(o.true_peak_dbtp), f(o.lra_lu))
                    });
                let (noise, conf, nr) =
                    report
                        .denoise
                        .as_ref()
                        .map_or(("".into(), "".into(), "".into()), |d| {
                            let src = match d.profile_source {
                                Some(leveled_audio_pipeline::ProfileSource::Minstat) => "MS",
                                Some(leveled_audio_pipeline::ProfileSource::Imported) => "imp",
                                _ => "",
                            };
                            let conf = if src.is_empty() {
                                d.confidence.map_or("–".into(), |c| format!("{c:.2}"))
                            } else {
                                src.to_string()
                            };
                            let nr = if d.skipped.is_some() {
                                "aus".into()
                            } else {
                                d.attenuation_pauses_db
                                    .map_or(String::new(), |a| format!("{a:.1}"))
                            };
                            (f(d.noise_level_dbfs), conf, nr)
                        });
                let lev = report.leveler.as_ref().map_or(String::new(), |l| {
                    if l.phrases == 0 {
                        "–".into()
                    } else {
                        format!("{:+.0}…{:+.0}", l.gain_min_db, l.gain_max_db)
                    }
                });
                let gain = report
                    .normalization_gain_db
                    .map_or(String::new(), |g| format!("{g:+.1}"));
                let lim = report
                    .limiter
                    .as_ref()
                    .map_or(String::new(), |l| format!("{:.1}", l.max_gain_reduction_db));
                println!(
                    "{name:<w$}  {:>6} {:>5} │ {noise:>6} {conf:>4} {nr:>5} │ {lev:>11} │ {oi:>6} {otp:>5} {olra:>4} │ {gain:>5} {lim:>4}  ok",
                    f(i.integrated_lufs),
                    f(i.true_peak_dbtp),
                );
                if let Some(reason) = report.denoise.as_ref().and_then(|d| d.skipped.as_ref()) {
                    notes.push(format!("  {}: Rauschreduktion aus – {reason}", r.display));
                }
                for warn in &report.warnings {
                    notes.push(format!("  {}: {warn}", r.display));
                }
            }
            Status::Skipped { .. } => {
                print!("{name:<w$}");
                blank("übersprungen (existiert)");
            }
            Status::Cancelled => {
                print!("{name:<w$}");
                blank("abgebrochen");
            }
            Status::Error { message } => {
                print!("{name:<w$}");
                blank("FEHLER");
                notes.push(format!("  {}: {message}", r.display));
            }
        }
    }
    println!(
        "\nRauschen: Profilpegel (dBFS), Konfidenz des Profils (MS = Minimum-Statistics-Ersatz, \
         imp = importiert), NR = Absenkung in Pausen (dB).\n\
         Leveler: Gain-Bereich in Sprache (dB). Gain: Normalisierung (dB), Lim: max. Limiter-Absenkung (dB)."
    );
    if !notes.is_empty() {
        println!("\nHinweise:");
        for n in notes {
            println!("{n}");
        }
    }
    let ok = results
        .iter()
        .filter(|r| matches!(r.status, Status::Ok { .. }))
        .count();
    let err = results.iter().filter(|r| r.is_error()).count();
    println!("\n{ok} ok, {err} Fehler, {} gesamt", results.len());
}

#[derive(Serialize)]
struct JsonReport<'a> {
    tool: &'static str,
    version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<&'a Settings>,
    files: &'a [FileOutcome],
}

pub fn write_json(path: &Path, files: &[FileOutcome], settings: Option<&Settings>) -> Result<()> {
    let rep = JsonReport {
        tool: "leveled-audio",
        version: env!("CARGO_PKG_VERSION"),
        settings,
        files,
    };
    let text = serde_json::to_string_pretty(&rep)?;
    std::fs::write(path, text).with_context(|| format!("JSON-Report {} schreiben", path.display()))
}
