//! Verarbeitungskette und Batch-Orchestrierung.
//!
//! Ablauf pro Datei (Mehrpass-Streaming, konstanter Speicher):
//! - **Analyse**: Lautheit, Clipping, DC, STFT-Features, VAD, Minimum Statistics;
//!   danach Rauschprofil (automatisch, Fallback oder importiert) und
//!   Leveler-Kurve.
//! - **Messen**: Lautheit nach Rauschreduktion und Leveler.
//! - **Rendern**: Rauschreduktion → Leveler → Gain → True-Peak-Limiter → Dither → Datei,
//!   dabei die Ausgabe messen.
//!
//! Die Pipeline kennt keine Fortschrittsbalken oder Signale; sie meldet über
//! [`ProgressSink`] und bricht über [`CancelToken`] ab. So kann eine spätere
//! GUI dieselbe API nutzen.

mod batch;
mod denoise_stage;
mod job;
mod leveler_stage;
mod noise;
mod progress;
mod report;
mod settings;

pub use batch::run_batch;
pub use job::{
    AlbumOutcome, JobOptions, analyze_file, learn_profile, process_album, process_file,
    resolve_output_spec,
};
pub use leveled_audio_analysis::{LevelerStats, LoudnessStats, NoiseProfile, ProfileSource};
pub use noise::{load_profile, save_profile};
pub use progress::{CancelToken, NoProgress, Pass, ProgressSink};
pub use report::{AlbumReport, DenoiseReport, FileReport, InputInfo, LimiterReport, OutputInfo};
pub use settings::{
    BitDepth, DenoiseSettings, DitherMode, FftSize, GainRuleSetting, LevelerSettings,
    LimiterSettings, LoudnessMode, LoudnessSettings, OutputSettings, PRESET_NAMES,
    PRESET_SCHEMA_VERSION, ProfileSettings, Settings, StatisticSetting,
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
