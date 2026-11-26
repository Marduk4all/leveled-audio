use leveled_audio_analysis::{Confidence, LevelerStats, LoudnessStats, ProfileSource};
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

/// Rauschreduktion: Profil und Wirkung.
#[derive(Debug, Clone, Serialize)]
pub struct DenoiseReport {
    /// Grund, falls die Rauschreduktion nicht angewendet wurde.
    pub skipped: Option<String>,
    pub profile_source: Option<ProfileSource>,
    /// Konfidenz des automatisch gesuchten Profils (0–1).
    pub confidence: Option<f64>,
    pub confidence_detail: Option<Confidence>,
    /// Menge des Pausenmaterials, aus dem das Profil stammt.
    pub profile_duration_s: f64,
    pub pauses_used: usize,
    /// Rauschpegel laut Profil (RMS, unbewertet).
    pub noise_level_dbfs: Option<f64>,
    /// Anteil der als Sprache erkannten Frames.
    pub speech_fraction: f64,
    pub fft_size: usize,
    pub gain_rule: String,
    pub reduction_db: f64,
    /// Gemessener Pegel in Pausen vor/nach der Rauschreduktion (dBFS).
    pub pause_level_in_dbfs: Option<f64>,
    pub pause_level_out_dbfs: Option<f64>,
    /// Mittlere Absenkung in Pausen bzw. in Sprache (dB, negativ).
    pub attenuation_pauses_db: Option<f64>,
    pub attenuation_speech_db: Option<f64>,
    pub delta_path: Option<PathBuf>,
    pub profile_saved: Option<PathBuf>,
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
    pub denoise: Option<DenoiseReport>,
    pub leveler: Option<LevelerStats>,
    /// Angewendeter Normalisierungs-Gain (inklusive Korrektur).
    pub normalization_gain_db: Option<f64>,
    /// Zusätzlicher Gain der Korrekturiteration, falls eine nötig war.
    pub loudness_correction_db: Option<f64>,
    pub limiter: Option<LimiterReport>,
    pub output: Option<OutputInfo>,
    pub loudness_out: Option<LoudnessStats>,
    pub warnings: Vec<String>,
    pub elapsed_s: f64,
    /// Audiodauer / Rechenzeit.
    pub realtime_factor: f64,
}

/// Album-Modus: gemeinsamer Gain und Album-Lautheit.
#[derive(Debug, Clone, Serialize)]
pub struct AlbumReport {
    /// Anzahl der Dateien, die in die Album-Lautheit eingehen.
    pub files: usize,
    /// Gemeinsame Lautheit nach Rauschreduktion und Leveler, vor dem Gain.
    pub loudness_before_gain_lufs: Option<f64>,
    /// Für alle Dateien gleicher Normalisierungs-Gain (inklusive Korrektur).
    pub gain_db: Option<f64>,
    pub loudness_correction_db: Option<f64>,
    /// Gemeinsame Lautheit der Ausgabe.
    pub loudness_out_lufs: Option<f64>,
    pub warnings: Vec<String>,
}
