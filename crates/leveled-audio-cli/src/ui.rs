//! Fortschrittsanzeige mit indicatif.

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use leveled_audio_pipeline::{Pass, ProgressSink};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Mutex;

pub struct Progress {
    multi: MultiProgress,
    overall: ProgressBar,
    bars: Mutex<HashMap<usize, ProgressBar>>,
    names: Vec<String>,
    /// Ohne Terminal: nur Zeilen „fertig: …“ ausgeben.
    plain: bool,
    quiet: bool,
}

impl Progress {
    pub fn new(names: &[String], quiet: bool) -> Self {
        let tty = std::io::stderr().is_terminal();
        let target = if quiet || !tty {
            ProgressDrawTarget::hidden()
        } else {
            ProgressDrawTarget::stderr()
        };
        let multi = MultiProgress::with_draw_target(target);
        let overall = multi.add(ProgressBar::new(names.len() as u64));
        overall.set_style(
            ProgressStyle::with_template(
                "{prefix:>8} [{bar:30.cyan/blue}] {pos}/{len} Dateien  {elapsed_precise}",
            )
            .expect("Vorlage")
            .progress_chars("=> "),
        );
        overall.set_prefix("Gesamt");
        Progress {
            multi,
            overall,
            bars: Mutex::new(HashMap::new()),
            names: names.to_vec(),
            plain: !tty,
            quiet,
        }
    }

    fn short_name(&self, id: usize) -> String {
        let n = &self.names[id];
        let chars: Vec<char> = n.chars().collect();
        if chars.len() > 40 {
            format!("…{}", chars[chars.len() - 39..].iter().collect::<String>())
        } else {
            n.clone()
        }
    }

    pub fn file_done(&self, id: usize) {
        if let Some(bar) = self.bars.lock().expect("Mutex").remove(&id) {
            bar.finish_and_clear();
        }
        self.overall.inc(1);
        if self.plain && !self.quiet {
            eprintln!("fertig: {}", self.names[id]);
        }
    }

    pub fn finish(&self) {
        self.overall.finish_and_clear();
        let _ = self.multi.clear();
    }
}

impl ProgressSink for Progress {
    fn file_started(&self, id: usize, frames: u64, passes: u32) {
        let bar = self
            .multi
            .insert_before(&self.overall, ProgressBar::new(frames * passes as u64));
        bar.set_style(
            ProgressStyle::with_template("{prefix:>8} [{bar:30}] {percent:>3}%  {msg}")
                .expect("Vorlage")
                .progress_chars("=> "),
        );
        bar.set_prefix(leveled_audio_pipeline::Pass::Analyze.label());
        bar.set_message(self.short_name(id));
        self.bars.lock().expect("Mutex").insert(id, bar);
    }

    fn pass_added(&self, id: usize, frames: u64) {
        if let Some(bar) = self.bars.lock().expect("Mutex").get(&id) {
            bar.inc_length(frames);
        }
    }

    fn file_finished(&self, id: usize) {
        if let Some(bar) = self.bars.lock().expect("Mutex").remove(&id) {
            bar.finish_and_clear();
        }
    }

    fn advance(&self, id: usize, pass: Pass, frames: u64) {
        if let Some(bar) = self.bars.lock().expect("Mutex").get(&id) {
            bar.set_prefix(pass.label());
            bar.inc(frames);
        }
    }
}
