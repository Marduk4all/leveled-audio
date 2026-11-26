//! Konsolenzusammenfassung und JSON-Report.

use anyhow::{Context, Result};
use leveled_audio_pipeline::{AlbumReport, FileReport, PipelineError, Settings};
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
        "{:<w$}  {:>6} {:>5} │ {:>6} {:>4} {:>5} {:>3} │ {:>11} │ {:>6} {:>5} {:>4} │ {:>5} {:>4} {:>4}  Status",
        "Datei",
        "LUFS",
        "dBTP",
        "dBFS",
        "Konf",
        "NR",
        "Kn",
        "Leveler",
        "LUFS",
        "dBTP",
        "LRA",
        "Gain",
        "Kmp",
        "Lim",
    );
    println!(
        "{:<w$}  {:─^12} │ {:─^21} │ {:─^11} │ {:─^17} │",
        "", " Eingang ", " Rauschen ", "", " Ausgang ",
    );
    let blank = |text: &str| println!("{text:>width$}", width = w + 93);
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
                let kn = report
                    .knock
                    .as_ref()
                    .map_or(String::new(), |k| k.regions.to_string());
                let kmp = report
                    .compressor
                    .as_ref()
                    .map_or(String::new(), |c| format!("{:.1}", c.max_gain_reduction_db));
                println!(
                    "{name:<w$}  {:>6} {:>5} │ {noise:>6} {conf:>4} {nr:>5} {kn:>3} │ {lev:>11} │ {oi:>6} {otp:>5} {olra:>4} │ {gain:>5} {kmp:>4} {lim:>4}  ok",
                    f(i.integrated_lufs),
                    f(i.true_peak_dbtp),
                );
                if let Some(reason) = report.denoise.as_ref().and_then(|d| d.skipped.as_ref()) {
                    notes.push(format!("  {}: Rauschreduktion aus – {reason}", r.display));
                }
                if let Some(g) = &report.gate {
                    notes.push(if g.attenuation_db >= 0.1 {
                        format!(
                            "  {}: Pausen um {:.1} dB abgesenkt (Ziel {:.0} dBFS, {:.0} % der Zeit)",
                            r.display,
                            g.attenuation_db,
                            g.floor_dbfs,
                            g.gated_fraction * 100.0
                        )
                    } else {
                        format!(
                            "  {}: Pausen liegen schon unter dem Ziel ({:.0} dBFS), keine Absenkung",
                            r.display, g.floor_dbfs
                        )
                    });
                }
                if let Some(c) = report.loudness_correction_db.filter(|c| c.abs() >= 0.5) {
                    notes.push(format!(
                        "  {}: Lautheit nach dem Limiter um {c:+.1} dB nachkorrigiert",
                        r.display
                    ));
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
         imp = importiert), NR = Absenkung in Pausen (dB), Kn = erkannte Knackser/Griffgeräusche.\n\
         Leveler: Gain-Bereich in Sprache (dB). Gain: Normalisierung (dB), Kmp/Lim: max. Absenkung durch\n\
         Kompressor/Limiter (dB)."
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

/// Version des JSON-Report-Formats (siehe `docs/report.schema.json`).
pub const REPORT_SCHEMA_VERSION: u32 = 2;

/// Kopfdaten des JSON-Reports.
pub struct JsonMeta<'a> {
    pub command: &'static str,
    pub settings: &'a Settings,
    pub album: Option<&'a AlbumReport>,
}

#[derive(Serialize)]
struct Counts {
    ok: usize,
    skipped: usize,
    errors: usize,
    cancelled: usize,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u32,
    tool: &'static str,
    version: &'static str,
    command: &'static str,
    settings: &'a Settings,
    /// Prüfsumme der effektiven Einstellungen: gleiche Prüfsumme = gleiche Verarbeitung.
    settings_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<&'a AlbumReport>,
    summary: Counts,
    files: &'a [FileOutcome],
}

/// FNV-1a (64 bit) über die kanonische JSON-Form der Einstellungen. Keine
/// kryptografische Prüfsumme, nur zum Wiedererkennen identischer Settings.
pub fn settings_hash(settings: &Settings) -> String {
    let json = serde_json::to_string(settings).expect("Settings sind serialisierbar");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{h:016x}")
}

pub fn write_json(path: &Path, files: &[FileOutcome], meta: JsonMeta) -> Result<()> {
    let count = |f: fn(&Status) -> bool| files.iter().filter(|o| f(&o.status)).count();
    let rep = JsonReport {
        schema_version: REPORT_SCHEMA_VERSION,
        tool: "leveled-audio",
        version: env!("CARGO_PKG_VERSION"),
        command: meta.command,
        settings: meta.settings,
        settings_hash: settings_hash(meta.settings),
        album: meta.album,
        summary: Counts {
            ok: count(|s| matches!(s, Status::Ok { .. })),
            skipped: count(|s| matches!(s, Status::Skipped { .. })),
            errors: count(|s| matches!(s, Status::Error { .. })),
            cancelled: count(|s| matches!(s, Status::Cancelled)),
        },
        files,
    };
    let mut text = serde_json::to_string_pretty(&rep)?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("JSON-Report {} schreiben", path.display()))
}

/// Album-Zeile vor der Tabelle.
pub fn print_album(a: &AlbumReport) {
    let f = |v: Option<f64>| v.map_or("–".into(), |v| format!("{v:.1}"));
    let corr = a
        .loudness_correction_db
        .map_or(String::new(), |c| format!(" (davon Korrektur {c:+.1} dB)"));
    println!(
        "Album ({} Dateien): {} LUFS vor dem Gain, gemeinsamer Gain {} dB{corr}, Ausgabe {} LUFS",
        a.files,
        f(a.loudness_before_gain_lufs),
        a.gain_db.map_or("–".into(), |g| format!("{g:+.1}")),
        f(a.loudness_out_lufs),
    );
    for w in &a.warnings {
        println!("  Hinweis: {w}");
    }
    println!();
}

/// Kurzbeschreibungen der Regler für `preset show` (PLAN 4.5).
const PRESET_COMMENTS: &[(&str, &str)] = &[
    (
        "highpass.frequency_hz",
        "Rumpeln/Trittschall weg; ab ≈ 100 Hz verlieren tiefe Stimmen Wärme",
    ),
    (
        "denoise.reduction_db",
        "max. Rauschabsenkung; ab ≈ 15–18 dB „blubbernde“ Artefakte",
    ),
    (
        "denoise.gain_rule",
        "wiener-dd = weniger Musical Noise, subtract = klassisch",
    ),
    (
        "knock.enabled",
        "Griffgeräusche/Knackser erkennen (werden nicht mitgelevelt)",
    ),
    (
        "knock.max_speech_to_low_db",
        "Sprachband so weit unter den Tiefen ⇒ Störgeräusch",
    ),
    (
        "knock.reduction_db",
        "Tiefen in Knacksern absenken; 0 = nur erkennen",
    ),
    (
        "leveler.strength",
        "0–1: 1 = Sprecher gleich laut, 0 = Unterschiede bleiben",
    ),
    ("leveler.max_boost_db", "größte Anhebung leiser Phrasen"),
    ("leveler.max_cut_db", "größte Absenkung lauter Phrasen"),
    (
        "gate.enabled",
        "Pausen-Absenkung (aus = natürlicher Raumton)",
    ),
    ("gate.floor_dbfs", "Pegel der Pausen in der fertigen Datei"),
    (
        "gate.max_attenuation_db",
        "höchstens so viel zusätzliche Absenkung",
    ),
    ("gate.hold_ms", "Halten nach dem letzten Laut"),
    (
        "gate.fade_out_ms",
        "Ausblenden in die Pause (länger = sanfter)",
    ),
    ("gate.fade_in_ms", "Aufblenden vor dem nächsten Wort"),
    (
        "compressor.amount",
        "0–1: 0,4 ≈ 1,5:1 (natürlich), 0,6 ≈ 2:1 (kräftig)",
    ),
    (
        "compressor.threshold_above_ref_db",
        "Schwelle über dem Sprachpegel der Datei",
    ),
    ("loudness.target_lufs", "Ziel-Lautheit"),
    (
        "loudness.mode",
        "per-file oder album (ein Gain für alle Dateien)",
    ),
    ("limiter.ceiling_dbtp", "Obergrenze für Spitzen"),
    ("output.bit_depth", "same, 16, 24 oder 32f"),
];

/// Ergänzt die TOML-Ausgabe von `preset show` um Kommentare.
pub fn annotate_preset(toml: &str) -> String {
    let mut out = String::from(
        "# leveled-audio-Preset. Nicht genannte Werte kommen vom Basis-Preset (`base`,\n\
         # Default podcast-stereo); Werte lassen sich auch einzeln mit --set ändern.\n",
    );
    let mut section = String::new();
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.trim_matches(|c| c == '[' || c == ']').to_string();
            out.push_str(line);
        } else if let Some((k, _)) = t.split_once(" = ") {
            let key = if section.is_empty() {
                k.to_string()
            } else {
                format!("{section}.{k}")
            };
            out.push_str(line);
            if let Some((_, c)) = PRESET_COMMENTS.iter().find(|(name, _)| *name == key) {
                out.push_str("  # ");
                out.push_str(c);
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}
