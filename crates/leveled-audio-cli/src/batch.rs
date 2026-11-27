//! Ausführung eines geplanten Stapels (pro Datei oder als Album), gemeinsam
//! für Kommandozeile und GUI.

use crate::inputs::Job;
use crate::summary::FileOutcome;
use leveled_audio_pipeline::{
    AlbumReport, CancelToken, JobOptions, LoudnessMode, Pass, ProgressSink, Settings,
    process_album, process_file, run_batch,
};

/// Leitet Fortschrittsmeldungen mit umgerechneter Datei-ID weiter (Album-Modus
/// zählt nur die zu verarbeitenden Dateien).
pub struct MappedProgress<'a> {
    pub inner: &'a dyn ProgressSink,
    pub ids: Vec<usize>,
}

impl ProgressSink for MappedProgress<'_> {
    fn file_started(&self, id: usize, frames: u64, passes: u32) {
        self.inner.file_started(self.ids[id], frames, passes);
    }
    fn advance(&self, id: usize, pass: Pass, frames: u64) {
        self.inner.advance(self.ids[id], pass, frames);
    }
    fn pass_added(&self, id: usize, frames: u64) {
        self.inner.pass_added(self.ids[id], frames);
    }
    fn file_finished(&self, id: usize) {
        self.inner.file_finished(self.ids[id]);
    }
}

/// Führt alle Aufträge aus. Ergebnisse in Planreihenfolge; `on_done(id)`
/// wird je Datei aufgerufen, sobald ihr Ergebnis feststeht. Im Album-Modus
/// kommt zusätzlich der Album-Report zurück.
pub fn run_plan(
    plan: Vec<Job>,
    settings: &Settings,
    opts: &JobOptions,
    jobs: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
    on_done: &(dyn Fn(usize) + Sync),
) -> (Vec<FileOutcome>, Option<AlbumReport>) {
    if settings.loudness.mode == LoudnessMode::Album {
        let (r, a) = run_album(plan, settings, opts, jobs, progress, cancel, on_done);
        return (r, Some(a));
    }
    let results = run_batch(plan, jobs, |id, job| {
        let outcome = match job {
            Job::Skip { input, output } => FileOutcome::skipped(input.display, input.path, output),
            Job::Process { input, output } => {
                let r = process_file(&input.path, &output, settings, opts, id, progress, cancel);
                FileOutcome::from_result(input.display, input.path, r)
            }
        };
        on_done(id);
        outcome
    });
    (results, None)
}

/// Album-Modus: alle zu verarbeitenden Dateien gemeinsam; übersprungene
/// Dateien zählen nicht zur Album-Lautheit.
fn run_album(
    plan: Vec<Job>,
    settings: &Settings,
    opts: &JobOptions,
    jobs: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
    on_done: &(dyn Fn(usize) + Sync),
) -> (Vec<FileOutcome>, AlbumReport) {
    let mut ids = Vec::new();
    let mut items = Vec::new();
    for (i, job) in plan.iter().enumerate() {
        if let Job::Process { input, output } = job {
            ids.push(i);
            items.push((input.path.clone(), output.clone()));
        }
    }
    let mapped = MappedProgress {
        inner: progress,
        ids,
    };
    let outcome = process_album(items, settings, opts, jobs, &mapped, cancel);
    let mut reports = outcome.files.into_iter();
    let mut album = outcome.album;
    let skipped = plan
        .iter()
        .filter(|j| matches!(j, Job::Skip { .. }))
        .count();
    if skipped > 0 {
        album.warnings.push(format!(
            "{skipped} übersprungene Datei(en) zählen nicht zur Album-Lautheit"
        ));
    }
    let results = plan
        .into_iter()
        .enumerate()
        .map(|(id, job)| {
            let o = match job {
                Job::Skip { input, output } => {
                    FileOutcome::skipped(input.display, input.path, output)
                }
                Job::Process { input, .. } => FileOutcome::from_result(
                    input.display,
                    input.path,
                    reports.next().expect("ein Ergebnis je Datei"),
                ),
            };
            on_done(id);
            o
        })
        .collect();
    (results, album)
}
