//! Einstellungen (serde, für TOML-Presets und JSON-Reports).
//!
//! Unbekannte Schlüssel sind ein Fehler, damit Tippfehler in Presets auffallen.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub loudness: LoudnessSettings,
    pub limiter: LimiterSettings,
    pub output: OutputSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoudnessSettings {
    pub enabled: bool,
    /// Ziel-Lautheit in LUFS.
    pub target_lufs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimiterSettings {
    pub enabled: bool,
    pub ceiling_dbtp: f64,
    pub lookahead_ms: f64,
    pub release_ms: f64,
    /// Oversampling der True-Peak-Detektion (mindestens 4).
    pub oversampling: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BitDepth {
    /// Wie die Eingabe (32-bit int wird zu 24-bit).
    Same,
    #[serde(rename = "16")]
    I16,
    #[serde(rename = "24")]
    I24,
    #[serde(rename = "32f")]
    F32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DitherMode {
    /// TPDF bei 16 bit, sonst aus.
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputSettings {
    pub bit_depth: BitDepth,
    pub dither: DitherMode,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::preset("podcast-stereo").expect("eingebautes Preset")
    }
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        LoudnessSettings {
            enabled: true,
            target_lufs: -16.0,
        }
    }
}

impl Default for LimiterSettings {
    fn default() -> Self {
        LimiterSettings {
            enabled: true,
            ceiling_dbtp: -1.0,
            lookahead_ms: 5.0,
            release_ms: 100.0,
            oversampling: 8,
        }
    }
}

impl Default for OutputSettings {
    fn default() -> Self {
        OutputSettings {
            bit_depth: BitDepth::Same,
            dither: DitherMode::Auto,
        }
    }
}

/// Namen der eingebauten Presets.
pub const PRESET_NAMES: [&str; 3] = ["podcast-stereo", "podcast-mono", "ebu-r128"];

impl Settings {
    /// Eingebaute Presets.
    pub fn preset(name: &str) -> Option<Settings> {
        let target = match name {
            "podcast-stereo" => -16.0,
            "podcast-mono" => -19.0,
            "ebu-r128" => -23.0,
            _ => return None,
        };
        Some(Settings {
            loudness: LoudnessSettings {
                enabled: true,
                target_lufs: target,
            },
            limiter: LimiterSettings::default(),
            output: OutputSettings::default(),
        })
    }

    /// Prüft Wertebereiche. Gibt eine Fehlermeldung für den Anwender zurück.
    pub fn validate(&self) -> std::result::Result<(), String> {
        let l = &self.loudness;
        if !(-70.0..=-5.0).contains(&l.target_lufs) {
            return Err(format!(
                "loudness.target_lufs = {} liegt außerhalb von −70 … −5 LUFS",
                l.target_lufs
            ));
        }
        let m = &self.limiter;
        if !(-20.0..=0.0).contains(&m.ceiling_dbtp) {
            return Err(format!(
                "limiter.ceiling_dbtp = {} liegt außerhalb von −20 … 0 dBTP",
                m.ceiling_dbtp
            ));
        }
        if !(0.5..=50.0).contains(&m.lookahead_ms) {
            return Err("limiter.lookahead_ms muss zwischen 0,5 und 50 ms liegen".into());
        }
        if !(1.0..=2000.0).contains(&m.release_ms) {
            return Err("limiter.release_ms muss zwischen 1 und 2000 ms liegen".into());
        }
        if ![4, 8, 16].contains(&m.oversampling) {
            return Err("limiter.oversampling muss 4, 8 oder 16 sein".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_exist_and_validate() {
        for name in PRESET_NAMES {
            let s = Settings::preset(name).unwrap();
            s.validate().unwrap();
        }
        assert_eq!(
            Settings::preset("ebu-r128").unwrap().loudness.target_lufs,
            -23.0
        );
    }

    #[test]
    fn toml_partial_and_unknown_keys() {
        let s: Settings =
            toml::from_str("[loudness]\ntarget_lufs = -19.0\n[output]\nbit_depth = \"32f\"\n")
                .unwrap();
        assert_eq!(s.loudness.target_lufs, -19.0);
        assert_eq!(s.output.bit_depth, BitDepth::F32);
        assert_eq!(s.limiter, LimiterSettings::default());

        let err = toml::from_str::<Settings>("[loudness]\ntarget_lfus = -19.0\n");
        assert!(err.is_err());
    }
}
