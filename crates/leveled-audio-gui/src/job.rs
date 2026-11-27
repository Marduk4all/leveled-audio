//! Auftrag der GUI ohne Oberfläche: Eingaben, Einstellungen → Stapel
//! ausführen. Nutzt dieselben Funktionen wie die Kommandozeile
//! (`leveled_audio_cli::inputs`, `leveled_audio_cli::batch`), damit beide bei gleichen
//! Einstellungen dieselben Dateien erzeugen.
//!
//! Ausgabe: immer neben der Originaldatei, als
//! `<Name>-leveled-<Preset>.<Endung>`.

use anyhow::{Result, bail};
use leveled_audio_cli::batch::run_plan;
use leveled_audio_cli::inputs::{InputFile, Job, collect_inputs};
use leveled_audio_cli::summary::FileOutcome;
use leveled_audio_pipeline::{AlbumReport, CancelToken, JobOptions, ProgressSink, Settings};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Kennzeichen exportierter Dateien im Dateinamen.
pub const SUFFIX: &str = "-leveled-";

/// Umgang mit bereits vorhandenen Zieldateien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Abbrechen, bevor etwas geschrieben wird.
    Error,
    Overwrite,
    Skip,
}

#[derive(Debug, Clone)]
pub struct Request {
    /// Dateien und Ordner, wie hinzugefügt.
    pub inputs: Vec<PathBuf>,
    pub recursive: bool,
    pub settings: Settings,
    /// Preset-Name für den Dateinamen.
    pub preset_label: String,
    pub jobs: usize,
}

/// Geplanter Stapel: Anzeigenamen und Aufträge.
pub struct Plan {
    pub names: Vec<String>,
    pub jobs: Vec<Job>,
}

/// Preset-Name als Teil eines Dateinamens (nur Buchstaben, Ziffern, `-`, `_`).
fn file_label(preset: &str) -> String {
    let s: String = preset
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "preset".into() } else { s }
}

/// Zieldatei neben der Eingabe: `aufnahme.wav` → `aufnahme-leveled-podcast-stereo.wav`.
pub fn output_path(input: &Path, preset: &str) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let ext = input
        .extension()
        .and_then(|s| s.to_str())
        .map_or(String::from(".wav"), |e| format!(".{e}"));
    input.with_file_name(format!("{stem}{SUFFIX}{}{ext}", file_label(preset)))
}

/// Eingaben auflösen. Bereits exportierte Dateien (`-leveled-` im Namen) aus
/// Ordnern werden übergangen, damit ein zweiter Lauf sie nicht erneut
/// bearbeitet; einzeln hinzugefügt werden sie verarbeitet.
fn files(inputs: &[PathBuf], recursive: bool) -> Result<Vec<InputFile>> {
    let explicit: HashSet<&PathBuf> = inputs.iter().filter(|p| p.is_file()).collect();
    Ok(collect_inputs(inputs, recursive)?
        .into_iter()
        .filter(|f| {
            explicit.contains(&f.path)
                || !f
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.contains(SUFFIX))
        })
        .collect())
}

/// Anzahl der WAV-Dateien in den Eingaben (für die Anzeige).
pub fn count_inputs(inputs: &[PathBuf], recursive: bool) -> Result<usize> {
    Ok(files(inputs, recursive)?.len())
}

/// Zieldateien, die schon existieren (für die Rückfrage vor dem Start).
pub fn existing_outputs(req: &Request) -> Result<Vec<PathBuf>> {
    Ok(files(&req.inputs, req.recursive)?
        .iter()
        .map(|f| output_path(&f.path, &req.preset_label))
        .filter(|o| o.exists())
        .collect())
}

/// Eingaben sammeln und Ausgabepfade prüfen (vor dem Start, ohne zu schreiben).
pub fn plan(req: &Request, existing: Existing) -> Result<Plan> {
    req.settings.validate().map_err(anyhow::Error::msg)?;
    if req.inputs.is_empty() {
        bail!("keine Eingaben");
    }
    let files = files(&req.inputs, req.recursive)?;
    if files.is_empty() {
        bail!("keine WAV-Dateien gefunden");
    }
    let mut jobs = Vec::new();
    let mut conflicts = Vec::new();
    for f in files {
        let output = output_path(&f.path, &req.preset_label);
        if output.exists() {
            match existing {
                Existing::Skip => {
                    jobs.push(Job::Skip { input: f, output });
                    continue;
                }
                Existing::Error => conflicts.push(output.display().to_string()),
                Existing::Overwrite => {}
            }
        }
        jobs.push(Job::Process { input: f, output });
    }
    if !conflicts.is_empty() {
        bail!(
            "{} Zieldatei(en) existieren bereits (z. B. {})",
            conflicts.len(),
            conflicts[0]
        );
    }
    Ok(Plan {
        names: jobs.iter().map(|j| j.display().to_string()).collect(),
        jobs,
    })
}

/// Ergebnis eines Laufs.
pub struct Outcome {
    pub files: Vec<FileOutcome>,
    pub album: Option<AlbumReport>,
}

/// Stapel ausführen (blockiert; die GUI ruft das in einem eigenen Thread auf).
pub fn execute(
    req: &Request,
    plan: Plan,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
    on_done: &(dyn Fn(usize) + Sync),
) -> Outcome {
    let (files, album) = run_plan(
        plan.jobs,
        &req.settings,
        &JobOptions::default(),
        req.jobs.max(1),
        progress,
        cancel,
        on_done,
    );
    Outcome { files, album }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_next_to_input() {
        assert_eq!(
            output_path(Path::new("/a/b/Abstand.WAV"), "podcast-stereo"),
            PathBuf::from("/a/b/Abstand-leveled-podcast-stereo.WAV")
        );
        assert_eq!(
            output_path(Path::new("x.wav"), "mein preset/1"),
            PathBuf::from("x-leveled-mein_preset_1.wav")
        );
    }
}
