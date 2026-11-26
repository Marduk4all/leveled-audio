//! Einstellungen (serde, für TOML-Presets und JSON-Reports).
//!
//! Unbekannte Schlüssel sind ein Fehler, damit Tippfehler in Presets auffallen.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub denoise: DenoiseSettings,
    pub leveler: LevelerSettings,
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

/// Adaptiver Leveler (gleicht unterschiedlich laute Sprecher/Phrasen an).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelerSettings {
    pub enabled: bool,
    /// 0–1 (0 = keine Wirkung, 1 = volle Angleichung an den Referenzpegel).
    pub strength: f64,
    pub max_boost_db: f64,
    pub max_cut_db: f64,
    /// Obergrenze des Gains in langen Pausen (≥ 2 s).
    pub pause_max_boost_db: f64,
    /// Maximale Gain-Änderung innerhalb einer Phrase.
    pub slew_db_per_s: f64,
}

impl Default for LevelerSettings {
    fn default() -> Self {
        LevelerSettings {
            enabled: true,
            strength: 0.6,
            max_boost_db: 10.0,
            max_cut_db: 10.0,
            pause_max_boost_db: 0.0,
            slew_db_per_s: 2.0,
        }
    }
}

impl LevelerSettings {
    /// Wirkt der Leveler überhaupt? (`strength = 0` ⇒ bitidentisch zum Bypass.)
    pub fn active(&self) -> bool {
        self.enabled && self.strength > 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoudnessSettings {
    pub enabled: bool,
    /// Ziel-Lautheit in LUFS.
    pub target_lufs: f64,
    /// Jede Datei einzeln oder ein gemeinsamer Gain für alle (Album).
    pub mode: LoudnessMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoudnessMode {
    /// Jede Datei trifft das Ziel.
    PerFile,
    /// Ein gemeinsamer Gain; die Album-Lautheit trifft das Ziel, relative
    /// Unterschiede zwischen den Dateien bleiben erhalten.
    Album,
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
            mode: LoudnessMode::PerFile,
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

/// Version des Preset-Formats.
pub const PRESET_SCHEMA_VERSION: i64 = 1;

fn merge(base: &mut toml::Table, patch: toml::Table) {
    for (k, v) in patch {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(p)) => merge(b, p),
            (_, v) => {
                base.insert(k, v);
            }
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
            leveler: LevelerSettings::default(),
            loudness: LoudnessSettings {
                target_lufs: target,
                ..LoudnessSettings::default()
            },
            limiter: LimiterSettings::default(),
            output: OutputSettings::default(),
        })
    }

    /// Preset-Datei (TOML) laden und schichten: eingebautes Basis-Preset
    /// (`base`, Default `podcast-stereo`) ← Werte aus der Datei. Die Datei darf
    /// unvollständig sein; unbekannte Schlüssel sind ein Fehler.
    ///
    /// Metadaten auf oberster Ebene: `schema_version` (muss 1 sein), `name`, `base`.
    pub fn from_preset_toml(text: &str) -> std::result::Result<Settings, String> {
        let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        match table.remove("schema_version") {
            None => {}
            Some(toml::Value::Integer(v)) if v == PRESET_SCHEMA_VERSION => {}
            Some(v) => {
                return Err(format!(
                    "schema_version = {v} wird nicht unterstützt (erwartet {PRESET_SCHEMA_VERSION})"
                ));
            }
        }
        match table.remove("name") {
            None | Some(toml::Value::String(_)) => {}
            Some(_) => return Err("name muss ein Text sein".into()),
        }
        let base = match table.remove("base") {
            None => "podcast-stereo".to_string(),
            Some(toml::Value::String(b)) => b,
            Some(_) => return Err("base muss ein Text sein".into()),
        };
        let base = Settings::preset(&base).ok_or_else(|| {
            format!(
                "base = \"{base}\": unbekanntes Preset (eingebaut: {})",
                PRESET_NAMES.join(", ")
            )
        })?;
        base.layered(table)
    }

    /// Einzelnen Wert überschreiben, z. B. `denoise.reduction_db=15` oder
    /// `loudness.mode=album`. Der Wert ist ein TOML-Wert; ohne Anführungszeichen
    /// wird er als Text genommen, wenn er sonst nicht lesbar ist.
    pub fn apply_override(&mut self, assignment: &str) -> std::result::Result<(), String> {
        let (key, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("„{assignment}“: erwartet SCHLÜSSEL=WERT"))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || key.split('.').any(str::is_empty) {
            return Err(format!("„{assignment}“: ungültiger Schlüssel"));
        }
        let parsed = toml::from_str::<toml::Table>(&format!("v = {value}"))
            .ok()
            .and_then(|mut t| t.remove("v"))
            .unwrap_or_else(|| toml::Value::String(value.to_string()));
        let mut patch = toml::Table::new();
        let parts: Vec<&str> = key.split('.').collect();
        let mut cur = &mut patch;
        for p in &parts[..parts.len() - 1] {
            cur = cur
                .entry(p.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .expect("Tabelle");
        }
        cur.insert(parts[parts.len() - 1].to_string(), parsed);
        *self = self
            .layered(patch)
            .map_err(|e| format!("--set {assignment}: {e}"))?;
        Ok(())
    }

    /// Als vollständige Preset-Datei ausgeben (Vorlage für eigene Presets).
    pub fn to_preset_toml(&self, name: &str) -> String {
        let body = toml::to_string_pretty(self).expect("Settings sind serialisierbar");
        format!(
            "schema_version = {PRESET_SCHEMA_VERSION}\nname = {}\n\n{body}",
            toml::Value::String(name.to_string())
        )
    }

    /// `patch` rekursiv über diese Einstellungen legen.
    fn layered(&self, patch: toml::Table) -> std::result::Result<Settings, String> {
        let mut base = toml::Table::try_from(self).map_err(|e| e.to_string())?;
        merge(&mut base, patch);
        toml::Value::Table(base)
            .try_into()
            .map_err(|e: toml::de::Error| e.message().to_string())
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
        let v = &self.leveler;
        if !(0.0..=1.0).contains(&v.strength) {
            return Err("leveler.strength muss zwischen 0 und 1 liegen (0–100 %)".into());
        }
        if !(0.0..=20.0).contains(&v.max_boost_db) || !(0.0..=20.0).contains(&v.max_cut_db) {
            return Err(
                "leveler.max_boost_db und max_cut_db müssen zwischen 0 und 20 dB liegen".into(),
            );
        }
        if !(-10.0..=10.0).contains(&v.pause_max_boost_db) {
            return Err("leveler.pause_max_boost_db muss zwischen −10 und +10 dB liegen".into());
        }
        if !(0.5..=10.0).contains(&v.slew_db_per_s) {
            return Err("leveler.slew_db_per_s muss zwischen 0,5 und 10 dB/s liegen".into());
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
    fn preset_layering() {
        // Basis ebu-r128, nur zwei Werte geändert; der Rest kommt vom Basis-Preset.
        let s = Settings::from_preset_toml(
            "schema_version = 1\nname = \"funk\"\nbase = \"ebu-r128\"\n\
             [denoise]\nreduction_db = 18.0\n[leveler]\nstrength = 0.3\n",
        )
        .unwrap();
        let mut expected = Settings::preset("ebu-r128").unwrap();
        expected.denoise.reduction_db = 18.0;
        expected.leveler.strength = 0.3;
        assert_eq!(s, expected);

        // Ohne base: podcast-stereo.
        let s = Settings::from_preset_toml("[loudness]\nmode = \"album\"\n").unwrap();
        assert_eq!(s.loudness.mode, LoudnessMode::Album);
        assert_eq!(s.loudness.target_lufs, -16.0);

        // Danach --set: überschreibt Datei-Werte, Rest bleibt.
        let mut s = Settings::from_preset_toml("[denoise]\nreduction_db = 18.0\n").unwrap();
        s.apply_override("denoise.reduction_db=9").unwrap();
        s.apply_override("loudness.mode = album").unwrap();
        s.apply_override("output.bit_depth=\"24\"").unwrap();
        s.apply_override("denoise.profile.statistic=median")
            .unwrap();
        assert_eq!(s.denoise.reduction_db, 9.0);
        assert_eq!(s.loudness.mode, LoudnessMode::Album);
        assert_eq!(s.output.bit_depth, BitDepth::I24);
        assert_eq!(s.denoise.profile.statistic, StatisticSetting::Median);
        assert_eq!(s.leveler, LevelerSettings::default());

        // Ausgabe von `preset show` lässt sich wieder laden.
        let text = Settings::preset("podcast-mono")
            .unwrap()
            .to_preset_toml("podcast-mono");
        assert_eq!(
            Settings::from_preset_toml(&text).unwrap(),
            Settings::preset("podcast-mono").unwrap()
        );
    }

    #[test]
    fn preset_errors() {
        let err = |t: &str| Settings::from_preset_toml(t).unwrap_err();
        assert!(err("[denoise]\nreduction_dB = 3.0\n").contains("reduction_dB"));
        assert!(err("[denose]\nenabled = false\n").contains("denose"));
        assert!(err("tempo = 3\n").contains("tempo"));
        assert!(err("schema_version = 2\n").contains("schema_version"));
        assert!(err("base = \"radio\"\n").contains("radio"));
        assert!(Settings::from_preset_toml("[loudness]\ntarget_lufs = \"laut\"\n").is_err());

        let mut s = Settings::default();
        let before = s.clone();
        assert!(
            s.apply_override("denoise.reduktion=3")
                .unwrap_err()
                .contains("reduktion")
        );
        assert!(s.apply_override("loudness.mode=radio").is_err());
        assert!(s.apply_override("limiter.ceiling_dbtp").is_err());
        assert!(s.apply_override("limiter..x=1").is_err());
        assert!(s.apply_override("denoise=3").is_err());
        assert_eq!(s, before, "fehlgeschlagenes --set ändert nichts");
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
        let bad: Settings = toml::from_str("[leveler]\nstrength = 1.5\n").unwrap();
        assert!(bad.validate().is_err());
        let bad: Settings = toml::from_str("[denoise]\nfft_size = 1000\n").unwrap();
        assert!(bad.validate().is_err());
        assert!(toml::from_str::<Settings>("[denoise]\nfft_size = \"gross\"\n").is_err());
    }
}
