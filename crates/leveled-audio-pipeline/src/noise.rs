//! Rauschprofil bestimmen: importiert, automatisch aus Pausen oder
//! Minimum-Statistics-Fallback. Dazu Laden und Speichern von Profilen.

use crate::job::InputAnalysis;
use crate::report::DenoiseReport;
use crate::settings::{DenoiseSettings, GainRuleSetting, StatisticSetting};
use crate::{PipelineError, Result};
use leveled_audio_analysis::{
    FrameClass, NoiseProfile, PROFILE_FORMAT_VERSION, ProfileBuilder, ProfileSearchParams,
    ProfileSource, ProfileStatistic, band_level_db, confidence, select_frames,
};
use leveled_audio_dsp::StftEngine;
use leveled_audio_io::AudioSource;
use std::path::Path;

/// Unterhalb dieses Rauschpegels lohnt sich keine Rauschreduktion.
const MIN_NOISE_DBFS: f64 = -95.0;
/// Abweichung (dB) zwischen importiertem Profil und Pausenpegel, ab der gewarnt wird.
const IMPORT_MISMATCH_DB: f32 = 6.0;

pub struct NoisePlan {
    /// Rauschleistung je Kanal und Bin; `None` ⇒ keine Rauschreduktion.
    pub power: Option<Vec<Vec<f32>>>,
    pub profile: Option<NoiseProfile>,
    pub report: DenoiseReport,
    pub warnings: Vec<String>,
}

fn statistic(s: StatisticSetting) -> ProfileStatistic {
    match s {
        StatisticSetting::Mean => ProfileStatistic::Mean,
        StatisticSetting::Trimmed => ProfileStatistic::Trimmed,
        StatisticSetting::Median => ProfileStatistic::Median,
    }
}

pub fn gain_rule_label(r: GainRuleSetting) -> &'static str {
    match r {
        GainRuleSetting::WienerDd => "wiener-dd",
        GainRuleSetting::Subtract => "subtract",
    }
}

fn empty_report(s: &DenoiseSettings, fft_size: usize, speech_fraction: f64) -> DenoiseReport {
    DenoiseReport {
        skipped: None,
        profile_source: None,
        confidence: None,
        confidence_detail: None,
        profile_duration_s: 0.0,
        pauses_used: 0,
        noise_level_dbfs: None,
        speech_fraction,
        fft_size,
        gain_rule: gain_rule_label(s.gain_rule).into(),
        reduction_db: s.reduction_db,
        pause_level_in_dbfs: None,
        pause_level_out_dbfs: None,
        attenuation_pauses_db: None,
        attenuation_speech_db: None,
        delta_path: None,
        profile_saved: None,
    }
}

/// Liest die Frames der Auswahl gezielt nach (Pass 1b) und baut das Profil.
fn build_profile(
    src: &mut dyn AudioSource,
    path: &Path,
    runs: &[(usize, usize)],
    fft_size: usize,
    stat: ProfileStatistic,
) -> Result<(Vec<Vec<f32>>, f64)> {
    let spec = src.spec();
    let ch = spec.channels as usize;
    let hop = fft_size / 4;
    let total = src.total_frames() as i64;
    let mut engine = StftEngine::new(ch, fft_size, false);
    let mut builder = ProfileBuilder::new(ch, fft_size / 2 + 1);
    let mut buf = vec![Vec::new(); ch];
    let mut window = vec![vec![0f32; fft_size]; ch];
    for &(a, b) in runs {
        // Frame k umfasst [(k+1)·hop − N, (k+1)·hop).
        let start = (a as i64 + 1) * hop as i64 - fft_size as i64;
        let end = b as i64 * hop as i64;
        let len = (end - start) as usize;
        let mut data = vec![vec![0f32; len]; ch];
        let rs = start.max(0);
        let re = end.min(total);
        if re > rs {
            src.seek(rs as u64).map_err(|e| PipelineError::Audio {
                path: path.to_path_buf(),
                source: e,
            })?;
            let mut off = (rs - start) as usize;
            let mut left = (re - rs) as usize;
            while left > 0 {
                let n = src
                    .read(&mut buf, left.min(65_536))
                    .map_err(|e| PipelineError::Audio {
                        path: path.to_path_buf(),
                        source: e,
                    })?;
                if n == 0 {
                    break;
                }
                for c in 0..ch {
                    data[c][off..off + n].copy_from_slice(&buf[c][..n]);
                }
                off += n;
                left -= n;
            }
        }
        for k in a..b {
            let o = (k - a) * hop;
            for c in 0..ch {
                window[c].copy_from_slice(&data[c][o..o + fft_size]);
            }
            builder.push(engine.spectrum_of(&window));
        }
    }
    Ok(builder.finish(stat, spec.sample_rate, fft_size))
}

/// Entscheidet über das Rauschprofil einer Datei.
pub fn plan_noise(
    src: &mut dyn AudioSource,
    path: &Path,
    a: &InputAnalysis,
    s: &DenoiseSettings,
    forced: Option<&NoiseProfile>,
) -> Result<NoisePlan> {
    let features = a
        .features
        .as_ref()
        .expect("Pass 1 mit STFT-Analyse gelaufen");
    let fft_size = features.fft_size;
    let rate = a.spec.sample_rate;
    let ch = a.spec.channels as usize;
    let n_frames = a.classes.len().max(1);
    let speech_fraction = a
        .classes
        .iter()
        .filter(|c| **c == FrameClass::Speech)
        .count() as f64
        / n_frames as f64;
    let mut report = empty_report(s, fft_size, speech_fraction);
    let mut warnings = Vec::new();

    // ---- Importiertes Profil
    if let Some(p) = forced {
        let power = p.power_for(rate, fft_size, ch);
        report.profile_source = Some(ProfileSource::Imported);
        report.confidence = Some(p.confidence);
        report.profile_duration_s = p.duration_s;
        report.pauses_used = p.pauses_used;
        report.noise_level_dbfs = Some(p.level_dbfs());
        // Plausibilität: Pausenpegel dieser Datei gegen das Profil.
        let mut pause_e: Vec<f32> = (0..features.len())
            .filter(|&k| a.classes[k] == FrameClass::Pause)
            .map(|k| features.e_db[k])
            .collect();
        if !pause_e.is_empty() {
            pause_e.sort_by(f32::total_cmp);
            let file_level = pause_e[pause_e.len() / 4];
            let prof_level = band_level_db(&power, rate, fft_size);
            if (file_level - prof_level).abs() > IMPORT_MISMATCH_DB {
                warnings.push(format!(
                    "importiertes Rauschprofil passt nicht zum Pausenpegel dieser Datei \
                     ({prof_level:.1} vs. {file_level:.1} dBFS); anderes Gain-Staging oder anderer Raum?"
                ));
            }
        }
        return Ok(NoisePlan {
            power: Some(power),
            profile: Some(p.clone()),
            report,
            warnings,
        });
    }

    // ---- Automatische Suche
    let params = ProfileSearchParams {
        min_duration_s: s.profile.min_duration_s,
        target_duration_s: s.profile.target_duration_s,
        ..Default::default()
    };
    let sel = select_frames(features, &a.classes, &params);
    let sel_dur = sel.frames.len() as f64 * features.hop_seconds();
    let stat = statistic(s.profile.statistic);

    let mut chosen: Option<(Vec<Vec<f32>>, ProfileSource)> = None;
    if sel_dur >= s.profile.min_duration_s {
        let (power, stationarity) = build_profile(src, path, &sel.runs(), fft_size, stat)?;
        let conf = confidence(
            sel_dur,
            s.profile.target_duration_s,
            sel.separation_db,
            stationarity,
            sel.pauses,
        );
        report.confidence = Some(conf.total);
        report.confidence_detail = Some(conf);
        report.profile_duration_s = sel_dur;
        report.pauses_used = sel.pauses;
        if conf.total >= s.profile.min_confidence {
            chosen = Some((power, ProfileSource::Auto));
        } else {
            warnings.push(format!(
                "Rauschprofil aus Pausen wenig verlässlich (Konfidenz {:.2}); \
                 Minimum Statistics als Ersatz",
                conf.total
            ));
        }
    } else {
        report.profile_duration_s = sel_dur;
        warnings.push(format!(
            "zu wenig ruhige Pausen für ein Rauschprofil ({sel_dur:.2} s < {:.2} s); \
             Minimum Statistics als Ersatz",
            s.profile.min_duration_s
        ));
    }
    if chosen.is_none()
        && let Some(ms) = &a.minstat
    {
        chosen = Some((ms.clone(), ProfileSource::Minstat));
    }
    let Some((power, source)) = chosen else {
        report.skipped = Some("kein Rauschprofil bestimmbar".into());
        return Ok(NoisePlan {
            power: None,
            profile: None,
            report,
            warnings,
        });
    };
    let mut profile = NoiseProfile::from_power(&power, rate, fft_size, source, stat);
    profile.confidence = report.confidence.unwrap_or(0.0);
    profile.duration_s = report.profile_duration_s;
    profile.pauses_used = report.pauses_used;
    let level = profile.level_dbfs();
    report.profile_source = Some(source);
    report.noise_level_dbfs = Some(level);
    if level < MIN_NOISE_DBFS {
        report.skipped = Some(format!(
            "Rauschpegel {level:.1} dBFS – praktisch rauschfrei, keine Reduktion nötig"
        ));
        return Ok(NoisePlan {
            power: None,
            profile: Some(profile),
            report,
            warnings,
        });
    }
    Ok(NoisePlan {
        power: Some(power),
        profile: Some(profile),
        report,
        warnings,
    })
}

/// Lädt ein Rauschprofil (JSON).
pub fn load_profile(path: &Path) -> Result<NoiseProfile> {
    let err = |m: String| PipelineError::Io {
        path: path.to_path_buf(),
        message: m,
    };
    let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
    let p: NoiseProfile =
        serde_json::from_str(&text).map_err(|e| err(format!("kein gültiges Rauschprofil: {e}")))?;
    if p.format_version != PROFILE_FORMAT_VERSION {
        return Err(err(format!(
            "Profilformat {} wird nicht unterstützt (erwartet {PROFILE_FORMAT_VERSION})",
            p.format_version
        )));
    }
    let bins = p.fft_size / 2 + 1;
    if p.bins_db.is_empty() || p.bins_db.iter().any(|c| c.len() != bins) || p.sample_rate == 0 {
        return Err(err(
            "Profil ist unvollständig (Bins passen nicht zur FFT-Größe)".into(),
        ));
    }
    Ok(p)
}

/// Speichert ein Rauschprofil (JSON).
pub fn save_profile(path: &Path, profile: &NoiseProfile) -> Result<()> {
    let text = serde_json::to_string(profile).expect("Profil serialisierbar");
    std::fs::write(path, text).map_err(|e| PipelineError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}
