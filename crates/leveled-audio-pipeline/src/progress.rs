use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Verarbeitungsdurchgang, für Fortschrittsanzeigen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    Analyze,
    Render,
}

impl Pass {
    pub fn label(self) -> &'static str {
        match self {
            Pass::Analyze => "Analyse",
            Pass::Render => "Rendern",
        }
    }
}

/// Empfängt Fortschrittsmeldungen. Muss threadsicher sein (Batch läuft parallel).
pub trait ProgressSink: Send + Sync {
    /// Eine Datei beginnt. `passes` Durchgänge à `frames` Frames folgen.
    fn file_started(&self, _id: usize, _frames: u64, _passes: u32) {}
    /// `frames` weitere Frames im Durchgang `pass` verarbeitet.
    fn advance(&self, id: usize, pass: Pass, frames: u64);
    fn file_finished(&self, _id: usize) {}
}

/// Verwirft alle Meldungen.
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn advance(&self, _id: usize, _pass: Pass, _frames: u64) {}
}

/// Kooperativer Abbruch: wird zwischen den Blöcken geprüft.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
