//! Analyse-Bausteine ohne Datei-I/O.
//!
//! Phase 1: Lautheitsmessung nach ITU-R BS.1770-4 / EBU R128.
//! Phase 2 ergänzt VAD, Rauschprofilsuche und Minimum Statistics.

pub mod loudness;

pub use loudness::{LoudnessMeter, LoudnessStats};
