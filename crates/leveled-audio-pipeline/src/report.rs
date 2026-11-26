use leveled_audio_analysis::LoudnessStats;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct InputInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: String,
    pub frames: u64,
    pub duration_s: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputInfo {
    pub path: PathBuf,
    pub format: String,
    /// Beim Quantisieren begrenzte Samples (soll 0 sein).
    pub clipped_samples: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LimiterReport {
    pub max_gain_reduction_db: f64,
    /// Anteil der Zeit, in der der Limiter eingreift (0–1).
    pub limited_fraction: f64,
}

/// Bericht für eine Datei. Felder, die ein Durchgang nicht ermittelt, sind `None`.
#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub input_path: PathBuf,
    pub input: InputInfo,
    pub loudness_in: LoudnessStats,
    /// Folgen von ≥ 3 Samples an der Vollaussteuerung in der Eingabe.
    pub input_clip_runs: u64,
    /// Gleichspannungsanteil je Kanal in dBFS (nur gemeldet, wenn > −50 dBFS).
    pub dc_offset_dbfs: Vec<Option<f64>>,
    pub normalization_gain_db: Option<f64>,
    pub limiter: Option<LimiterReport>,
    pub output: Option<OutputInfo>,
    pub loudness_out: Option<LoudnessStats>,
    pub warnings: Vec<String>,
    pub elapsed_s: f64,
    /// Audiodauer / Rechenzeit.
    pub realtime_factor: f64,
}
