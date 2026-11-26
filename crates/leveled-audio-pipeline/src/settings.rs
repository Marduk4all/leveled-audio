//! Einstellungen (serde, für TOML-Presets und JSON-Reports).
//!
//! Unbekannte Schlüssel sind ein Fehler, damit Tippfehler in Presets auffallen.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub denoise: DenoiseSettings,
    pub loudness: LoudnessSettings,
    pub limiter: LimiterSettings,
    pub output: OutputSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DenoiseSettings {
    pub enabled: bool,
    pub gain_rule: GainRuleSetting,
    /// Maximale Absenkung in dB.
    pub reduction_db: f64,
    pub oversubtraction: f64,
    /// Spectral Floor β relativ zum Rauschprofil (nur `subtract`).
    pub floor: f64,
    pub release_ms: f64,
    /// Frequenzglättung des Gains in Oktaven (0 = aus).
    pub freq_smooth_oct: f64,
    pub fft_size: FftSize,
    /// Gemeinsamer Gain für beide Kanäle (erhält das Stereobild).
    pub stereo_link: bool,
    pub profile: ProfileSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileSettings {
    /// Mindestmenge Pausenmaterial für ein automatisches Profil.
    pub min_duration_s: f64,
    /// Ab dieser Menge gilt die Menge als voll ausreichend.
    pub target_duration_s: f64,
    pub statistic: StatisticSetting,
    /// Unterhalb dieser Konfidenz wird Minimum Statistics verwendet.
    pub min_confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GainRuleSetting {
    WienerDd,
    Subtract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatisticSetting {
    Mean,
    Trimmed,
    Median,
}

/// FFT-Größe: `"auto"` (≈ 43 ms) oder eine Zweierpotenz 512 … 8192.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FftSize {
    Auto,
    Fixed(usize),
}

impl FftSize {
    pub fn resolve(self, sample_rate: u32) -> usize {
        match self {
            FftSize::Auto => leveled_audio_dsp::auto_fft_size(sample_rate),
            FftSize::Fixed(n) => n,
        }
    }
}

impl Serialize for FftSize {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            FftSize::Auto => s.serialize_str("auto"),
            FftSize::Fixed(n) => s.serialize_u64(*n as u64),
        }
    }
}

impl<'de> Deserialize<'de> for FftSize {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(usize),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Num(n) => Ok(FftSize::Fixed(n)),
            Raw::Str(s) if s == "auto" => Ok(FftSize::Auto),
            Raw::Str(s) => Err(serde::de::Error::custom(format!(
                "fft_size muss \"auto\" oder eine Zahl sein, nicht \"{s}\""
            ))),
        }
    }
}

impl Default for DenoiseSettings {
    fn default() -> Self {
        DenoiseSettings {
            enabled: true,
            gain_rule: GainRuleSetting::WienerDd,
            reduction_db: 12.0,
            oversubtraction: 1.3,
            floor: 0.05,
            release_ms: 40.0,
            freq_smooth_oct: 0.0,
            fft_size: FftSize::Auto,
            stereo_link: false,
            profile: ProfileSettings::default(),
        }
    }
}

impl Default for ProfileSettings {
    fn default() -> Self {
        ProfileSettings {
            min_duration_s: 0.3,
            target_duration_s: 2.0,
            statistic: StatisticSetting::Trimmed,
            min_confidence: 0.4,
        }
    }
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
            denoise: DenoiseSettings::default(),
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
        let d = &self.denoise;
        if !(0.0..=40.0).contains(&d.reduction_db) {
            return Err("denoise.reduction_db muss zwischen 0 und 40 dB liegen".into());
        }
        if !(0.5..=4.0).contains(&d.oversubtraction) {
            return Err("denoise.oversubtraction muss zwischen 0,5 und 4 liegen".into());
        }
        if !(0.0..=1.0).contains(&d.floor) {
            return Err("denoise.floor muss zwischen 0 und 1 liegen".into());
        }
        if !(0.0..=1000.0).contains(&d.release_ms) {
            return Err("denoise.release_ms muss zwischen 0 und 1000 ms liegen".into());
        }
        if !(0.0..=1.0).contains(&d.freq_smooth_oct) {
            return Err("denoise.freq_smooth_oct muss zwischen 0 und 1 Oktave liegen".into());
        }
        if let FftSize::Fixed(n) = d.fft_size
            && (!n.is_power_of_two() || !(512..=8192).contains(&n))
        {
            return Err(format!(
                "denoise.fft_size = {n}: erlaubt sind \"auto\" oder 512, 1024, 2048, 4096, 8192"
            ));
        }
        let pr = &d.profile;
        if !(0.1..=60.0).contains(&pr.min_duration_s) || pr.target_duration_s < pr.min_duration_s {
            return Err(
                "denoise.profile: min_duration_s (0,1–60 s) darf target_duration_s nicht übersteigen"
                    .into(),
            );
        }
        if !(0.0..=1.0).contains(&pr.min_confidence) {
            return Err("denoise.profile.min_confidence muss zwischen 0 und 1 liegen".into());
        }
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

    #[test]
    fn denoise_settings_toml() {
        let s: Settings = toml::from_str(
            "[denoise]\ngain_rule = \"subtract\"\nfft_size = 1024\n[denoise.profile]\nstatistic = \"median\"\n",
        )
        .unwrap();
        assert_eq!(s.denoise.gain_rule, GainRuleSetting::Subtract);
        assert_eq!(s.denoise.fft_size, FftSize::Fixed(1024));
        assert_eq!(s.denoise.profile.statistic, StatisticSetting::Median);
        s.validate().unwrap();
        let back: Settings =
            toml::from_str(&toml::to_string(&Settings::default()).unwrap()).unwrap();
        assert_eq!(back, Settings::default());
        let bad: Settings = toml::from_str("[denoise]\nfft_size = 1000\n").unwrap();
        assert!(bad.validate().is_err());
        assert!(toml::from_str::<Settings>("[denoise]\nfft_size = \"gross\"\n").is_err());
    }
}
