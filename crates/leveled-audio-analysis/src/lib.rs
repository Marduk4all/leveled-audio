//! Analyse-Bausteine ohne Datei-I/O.
//!
//! - Lautheitsmessung nach ITU-R BS.1770-4 / EBU R128
//! - Frame-Features, Sprach-/Pausenerkennung (VAD)
//! - Automatische Rauschprofilsuche mit Konfidenz, Minimum-Statistics-Fallback

pub mod features;
pub mod loudness;
pub mod minstat;
pub mod profile;
pub mod vad;

pub use features::{FeatureExtractor, FrameFeatures, band_level_db, power_scale};
pub use loudness::{LoudnessMeter, LoudnessStats};
pub use minstat::MinStatTracker;
pub use profile::{
    Confidence, NoiseProfile, PROFILE_FORMAT_VERSION, ProfileBuilder, ProfileSearchParams,
    ProfileSource, ProfileStatistic, Selection, confidence, select_frames,
};
pub use vad::{FrameClass, VadParams, classify};
