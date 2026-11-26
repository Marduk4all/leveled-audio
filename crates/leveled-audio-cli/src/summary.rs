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
        .clamp(5, 48);
    println!(
        "{:<w$}  {:>7} {:>6} {:>5}   {:>7} {:>6} {:>5}  {:>6}  {:>5}  Status",
        "Datei", "LUFS", "dBTP", "LRA", "LUFS", "dBTP", "LRA", "Gain", "Lim.",
    );
    println!("{:<w$}  {:─^20}   {:─^20}", "", " Eingang ", " Ausgang ",);
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
                let gain = report
                    .normalization_gain_db
                    .map_or(String::new(), |g| format!("{g:+.1}"));
                let lim = report
                    .limiter
                    .as_ref()
                    .map_or(String::new(), |l| format!("{:.1}", l.max_gain_reduction_db));
                println!(
                    "{name:<w$}  {:>7} {:>6} {:>5}   {oi:>7} {otp:>6} {olra:>5}  {gain:>6}  {lim:>5}  ok",
                    f(i.integrated_lufs),
                    f(i.true_peak_dbtp),
                    f(i.lra_lu),
                );
                for warn in &report.warnings {
                    notes.push(format!("  {}: {warn}", r.display));
                }
            }
            Status::Skipped { .. } => println!("{name:<w$}  {:>74}", "übersprungen (existiert)"),
            Status::Cancelled => println!("{name:<w$}  {:>74}", "abgebrochen"),
            Status::Error { message } => {
                println!("{name:<w$}  {:>74}", "FEHLER");
                notes.push(format!("  {}: {message}", r.display));
            }
        }
    }
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
