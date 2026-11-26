//! Verarbeitungskette und Batch-Orchestrierung.
//!
//! Ablauf pro Datei (Mehrpass-Streaming, konstanter Speicher):
//! - **Analyse**: Eingabe lesen und messen (Lautheit, Clipping, DC).
//!   Ab Phase 2 kommen hier Features, VAD und Rauschprofil hinzu.
//! - **Render**: Eingabe → Gain → True-Peak-Limiter → Dither → Datei,
//!   dabei die Ausgabe messen.
//!
//! Die Pipeline kennt keine Fortschrittsbalken oder Signale; sie meldet über
//! [`ProgressSink`] und bricht über [`CancelToken`] ab. So kann eine spätere
//! GUI dieselbe API nutzen.

mod batch;
mod job;
mod progress;
mod report;
mod settings;

pub use batch::run_batch;
pub use job::{analyze_file, process_file, resolve_output_spec};
pub use leveled_audio_analysis::LoudnessStats;
pub use progress::{CancelToken, NoProgress, Pass, ProgressSink};
pub use report::{FileReport, InputInfo, LimiterReport, OutputInfo};
pub use settings::{
    BitDepth, DitherMode, LimiterSettings, LoudnessSettings, OutputSettings, PRESET_NAMES, Settings,
};

use std::path::PathBuf;

/// Blockgröße der Verarbeitung in Frames.
pub const BLOCK_FRAMES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("{path}: {source}")]
    Audio {
        path: PathBuf,
        #[source]
        source: leveled_audio_io::IoError,
    },
    #[error("{path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("{0}")]
    Analysis(String),
    #[error("abgebrochen")]
    Cancelled,
}

impl PipelineError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, PipelineError::Cancelled)
    }
}

pub type Result<T> = std::result::Result<T, PipelineError>;
