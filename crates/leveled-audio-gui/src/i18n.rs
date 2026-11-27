//! Sprache der Oberfläche: Englisch (Standard) oder Deutsch.
//!
//! Die Texte der Pipeline (Parameter, Gruppen, Presets, Durchgänge) sind
//! deutsch; hier stehen die englischen Fassungen, nach Schlüssel bzw.
//! deutschem Text. Ein Test stellt sicher, dass nichts fehlt.

use leveled_audio_pipeline::ParamSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    De,
}

impl Lang {
    /// Text in der gewählten Sprache.
    pub fn t(self, de: &'static str, en: &'static str) -> &'static str {
        match self {
            Lang::De => de,
            Lang::En => en,
        }
    }

    /// Bezeichnung und Hilfetext eines Parameters.
    pub fn param(self, p: &ParamSpec) -> (&'static str, &'static str) {
        match self {
            Lang::De => (p.label, p.help),
            Lang::En => param_en(p.key).unwrap_or((p.label, p.help)),
        }
    }

    /// Gruppe (Stufe der Kette).
    pub fn group(self, de: &'static str) -> &'static str {
        match self {
            Lang::De => de,
            Lang::En => group_en(de).unwrap_or(de),
        }
    }

    /// Bezeichnung eines Auswahlwerts.
    pub fn choice(self, de: &'static str) -> &'static str {
        match self {
            Lang::De => de,
            Lang::En => choice_en(de).unwrap_or(de),
        }
    }

    /// Kurzbeschreibung eines eingebauten Presets.
    pub fn preset(self, name: &str) -> &'static str {
        match self {
            Lang::De => leveled_audio_pipeline::preset_description(name),
            Lang::En => preset_en(name),
        }
    }

    /// Durchgang (Analyse, Messen, Rendern).
    pub fn pass(self, de: &'static str) -> &'static str {
        match (self, de) {
            (Lang::En, "Analyse") => "Analyzing",
            (Lang::En, "Messen") => "Measuring",
            (Lang::En, "Rendern") => "Rendering",
            _ => de,
        }
    }
}

fn group_en(de: &str) -> Option<&'static str> {
    Some(match de {
        "Hochpass" => "High-pass",
        "Rauschreduktion" => "Noise reduction",
        "Knackser" => "Handling noise",
        "Leveler" => "Leveler",
        "Pausen-Absenkung" => "Pause gate",
        "Kompressor" => "Compressor",
        "Lautheit" => "Loudness",
        "Limiter" => "Limiter",
        "Ausgabe" => "Output",
        _ => return None,
    })
}

fn choice_en(de: &str) -> Option<&'static str> {
    Some(match de {
        "Wiener (weniger Musical Noise)" => "Wiener (less musical noise)",
        "Power Subtraction (klassisch)" => "Power subtraction (classic)",
        "getrimmtes Mittel" => "trimmed mean",
        "Mittel" => "mean",
        "Median" => "median",
        "pro Datei" => "per file",
        "Album (ein Gain für alle)" => "album (one gain for all)",
        "wie Eingabe" => "same as input",
        "16 bit" => "16 bit",
        "24 bit" => "24 bit",
        "32 bit float" => "32 bit float",
        "automatisch (nur 16 bit)" => "automatic (16 bit only)",
        "an" => "on",
        "aus" => "off",
        _ => return None,
    })
}

fn preset_en(name: &str) -> &'static str {
    match name {
        "podcast-stereo" => "natural, −16 LUFS (default)",
        "podcast-mono" => "natural, −19 LUFS (mono podcast)",
        "podcast-kraeftig" => "punchy: near-silent pauses, denser, −16 LUFS",
        "ebu-r128" => "broadcast, −23 LUFS, natural room tone",
        _ => "",
    }
}

fn param_en(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "highpass.enabled" => (
            "High-pass on",
            "Removes rumble and footstep noise below the cutoff frequency.",
        ),
        "highpass.frequency_hz" => (
            "Cutoff frequency",
            "Higher = leaner; above ≈ 100 Hz deep voices lose warmth.",
        ),
        "denoise.enabled" => (
            "Noise reduction on",
            "Noise profile taken automatically from the quietest pauses and subtracted spectrally. Off: no handling-noise reduction either.",
        ),
        "denoise.reduction_db" => (
            "Max. reduction",
            "Less noise; above ≈ 15–18 dB “bubbly” artifacts and duller speech become likely.",
        ),
        "denoise.gain_rule" => (
            "Gain rule",
            "Wiener filter with decision-directed estimation sounds calmer.",
        ),
        "denoise.oversubtraction" => (
            "Oversubtraction",
            "How much the noise profile is overestimated. Higher = more thorough, but more artifacts.",
        ),
        "denoise.floor" => (
            "Spectral floor",
            "Lower limit relative to the noise (power subtraction only).",
        ),
        "denoise.release_ms" => (
            "Release",
            "How fast the reduction returns after speech. Longer = calmer, but more smeared.",
        ),
        "denoise.freq_smooth_oct" => (
            "Frequency smoothing",
            "Smooths the gain across frequency; reduces musical noise, makes the reduction coarser.",
        ),
        "denoise.fft_size" => (
            "FFT size",
            "auto ≈ 43 ms. Larger = finer frequency resolution, but blurred transients.",
        ),
        "denoise.stereo_link" => (
            "Stereo linked",
            "Same gain for both channels; keeps the stereo image.",
        ),
        "denoise.profile.min_duration_s" => (
            "Profile: min. duration",
            "Minimum amount of pause material an automatic profile needs.",
        ),
        "denoise.profile.target_duration_s" => (
            "Profile: target duration",
            "From this amount of pause material on, the profile is fully trusted (≥ min. duration).",
        ),
        "denoise.profile.statistic" => (
            "Profile: averaging",
            "How the pauses are averaged into the profile.",
        ),
        "denoise.profile.min_confidence" => (
            "Profile: min. confidence",
            "Below this, minimum statistics replace the profile.",
        ),
        "knock.enabled" => (
            "Detect handling noise",
            "Detects handling noise and knocks; they are then not boosted by the leveler.",
        ),
        "knock.reduction_db" => (
            "Low-end reduction",
            "Quieter knocks; simultaneous speech briefly sounds thinner there. 0 = detect only.",
        ),
        "knock.max_speech_to_low_db" => (
            "Detection threshold",
            "Speech band this far below the lows ⇒ noise. Higher detects more, but risks deep speech sounds.",
        ),
        "leveler.enabled" => (
            "Leveler on",
            "Evens out speakers and phrases of different loudness.",
        ),
        "leveler.strength" => (
            "Strength",
            "100 % = speakers equally loud, 0 % = differences remain. Above 100 % overcorrects: quiet passages end up slightly above loud ones, which compensates more for the leveler's smoothing and large distance changes. Higher sounds more even, but less lively.",
        ),
        "leveler.max_boost_db" => ("Max. boost", "Largest boost of quiet phrases."),
        "leveler.max_cut_db" => ("Max. cut", "Largest cut of loud phrases."),
        "leveler.pause_max_boost_db" => (
            "Max. gain in pauses",
            "Upper limit of the gain in long pauses; 0 = pauses are not raised.",
        ),
        "leveler.slew_db_per_s" => (
            "Max. change within phrases",
            "How fast the gain may change within a phrase. Higher = faster, but risk of pumping.",
        ),
        "gate.enabled" => (
            "Lower pauses",
            "Off = natural room tone stays. On = pauses nearly silent; may “breathe” at pause edges.",
        ),
        "gate.floor_dbfs" => (
            "Pause level",
            "Target level of pauses in the finished file.",
        ),
        "gate.max_attenuation_db" => (
            "Max. attenuation",
            "At most this much additional attenuation.",
        ),
        "gate.min_pause_ms" => (
            "Min. pause",
            "Shorter pauses stay untouched (otherwise speech gets chopped between words).",
        ),
        "gate.hold_ms" => (
            "Hold",
            "Hold after the last sound before fading out. Longer protects decays.",
        ),
        "gate.fade_out_ms" => (
            "Fade out",
            "Fade into the pause. Longer = smoother, but quiet later.",
        ),
        "gate.fade_in_ms" => ("Fade in", "Fade in before the next word."),
        "gate.pre_open_ms" => (
            "Pre-open",
            "The attenuation is fully lifted this much before speech starts.",
        ),
        "compressor.enabled" => (
            "Compressor on",
            "Catches short loud passages (laughter, exclamations) before the limiter clamps hard.",
        ),
        "compressor.amount" => (
            "Strength",
            "40 % ≈ 1.5:1 (natural), 60 % ≈ 2:1 (punchy). Higher = denser, too high sounds squashed.",
        ),
        "compressor.threshold_above_ref_db" => (
            "Threshold above speech",
            "Threshold relative to the file's speech level. Lower also catches normal syllables.",
        ),
        "compressor.attack_ms" => ("Attack", "How fast the compressor grabs."),
        "compressor.release_ms" => (
            "Release",
            "How fast it lets go again. Shorter = livelier, but risk of pumping.",
        ),
        "loudness.enabled" => (
            "Normalize",
            "Brings the integrated loudness (EBU R128) to the target.",
        ),
        "loudness.target_lufs" => (
            "Target loudness",
            "Overall loudness. Platforms usually normalize to −14 to −16 LUFS.",
        ),
        "loudness.mode" => (
            "Mode",
            "Album only for episodes of one production; otherwise quiet files stay quiet.",
        ),
        "limiter.enabled" => ("Limiter on", "True-peak limiter with oversampling."),
        "limiter.ceiling_dbtp" => (
            "Ceiling",
            "Upper limit for peaks. Higher relieves the limiter, but risks distortion after MP3/AAC.",
        ),
        "limiter.lookahead_ms" => ("Lookahead", "How far ahead the limiter looks."),
        "limiter.release_ms" => ("Release", "How fast the limiter lets go again."),
        "limiter.oversampling" => (
            "Oversampling",
            "Accuracy of peak detection between samples.",
        ),
        "output.bit_depth" => ("Bit depth", "“same as input” turns 32-bit int into 24 bit."),
        "output.dither" => ("Dither", "TPDF dither when reducing the bit depth."),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use leveled_audio_pipeline::{PARAM_GROUPS, PARAMS, PRESET_NAMES, ParamKind, Pass};

    #[test]
    fn every_text_has_english() {
        for p in PARAMS {
            assert!(param_en(p.key).is_some(), "Parameter {}", p.key);
            if let ParamKind::Choice(opts) = p.kind {
                for (_, l) in opts {
                    assert!(choice_en(l).is_some(), "Auswahl {l}");
                }
            }
        }
        for g in PARAM_GROUPS {
            assert!(group_en(g).is_some(), "Gruppe {g}");
        }
        for n in PRESET_NAMES {
            assert!(!preset_en(n).is_empty(), "Preset {n}");
        }
        for p in [Pass::Analyze, Pass::Measure, Pass::Render] {
            assert_ne!(Lang::En.pass(p.label()), p.label());
        }
    }
}
