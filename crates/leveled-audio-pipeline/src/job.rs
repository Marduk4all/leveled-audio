//! Verarbeitung einer einzelnen Datei.
//!
//! Passes (jeweils streamend, konstanter Speicher):
//! 1. **Analyse**: Lautheit, Clipping, DC; mit Rauschreduktion zusätzlich
//!    STFT-Features, VAD und Minimum Statistics.
//!    Danach offline: Rauschprofil bestimmen (gezieltes Nachlesen der Pausen).
//! 2. **Messen** (nur mit Rauschreduktion): Lautheit nach dem Entrauschen.
//! 3. **Rendern**: Entrauschen → Gain → True-Peak-Limiter → Dither → Datei.

use crate::denoise_stage::DenoiseStage;
use crate::noise::{NoisePlan, plan_noise, save_profile};
use crate::progress::{CancelToken, Pass, ProgressSink};
use crate::report::{FileReport, InputInfo, LimiterReport, OutputInfo};
use crate::settings::{BitDepth, DenoiseSettings, DitherMode, GainRuleSetting, Settings};
use crate::{BLOCK_FRAMES, PipelineError, Result};
use leveled_audio_analysis::{
    FeatureExtractor, FrameClass, FrameFeatures, LoudnessMeter, LoudnessStats, MinStatTracker,
    NoiseProfile, VadParams, classify,
};
use leveled_audio_dsp::{
    DelayLine, DenoiseParams, GainRule, LimiterParams, StftEngine, TpdfDither, TruePeakLimiter,
    apply_gain, db_to_lin,
};
use leveled_audio_io::{AudioSink, AudioSource, AudioSpec, SampleFormat};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Ab dieser Aussteuerung gilt ein Sample als „an der Grenze“ (1 LSB bei 16 bit).
const CLIP_LEVEL: f32 = 1.0 - 1.0 / 32768.0;
const CLIP_RUN: u32 = 3;
/// Minimum-Statistics-Fenster und Abstand der Momentaufnahmen.
const MINSTAT_WINDOW_S: f64 = 2.5;
const MINSTAT_MAX_SNAPSHOTS: f64 = 1000.0;

/// Optionen, die nicht Teil eines Presets sind (pro Aufruf).
#[derive(Debug, Clone, Default)]
pub struct JobOptions {
    /// Erzwungenes Rauschprofil für alle Dateien.
    pub noise_profile: Option<Arc<NoiseProfile>>,
    /// Automatisch bestimmte Profile als `<stem>.noise.json` hier ablegen.
    pub save_profile_dir: Option<PathBuf>,
    /// Entfernten Anteil als `<ausgabe>.delta.wav` (32 bit float) schreiben.
    pub export_delta: bool,
}

pub(crate) struct InputAnalysis {
    pub spec: AudioSpec,
    pub frames: u64,
    pub loudness: LoudnessStats,
    pub clip_runs: u64,
    pub dc_dbfs: Vec<Option<f64>>,
    pub features: Option<FrameFeatures>,
    pub classes: Vec<FrameClass>,
    pub minstat: Option<Vec<Vec<f32>>>,
}

fn audio_err(path: &Path) -> impl FnOnce(leveled_audio_io::IoError) -> PipelineError + '_ {
    move |source| PipelineError::Audio {
        path: path.to_path_buf(),
        source,
    }
}

fn analysis_err(e: impl std::fmt::Display) -> PipelineError {
    PipelineError::Analysis(e.to_string())
}

fn open(path: &Path) -> Result<Box<dyn AudioSource>> {
    leveled_audio_io::open_source(path).map_err(audio_err(path))
}

fn fft_size_for(d: &DenoiseSettings, rate: u32) -> usize {
    d.fft_size.resolve(rate)
}

/// Pass 1: Eingabe messen; mit `stft_size` zusätzlich Features/VAD/Minimum Statistics.
fn analysis_pass(
    src: &mut dyn AudioSource,
    path: &Path,
    stft_size: Option<usize>,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<InputAnalysis> {
    let spec = src.spec();
    let ch = spec.channels as usize;
    let total = src.total_frames();
    let mut meter = LoudnessMeter::new(ch, spec.sample_rate).map_err(analysis_err)?;
    let mut buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut run = vec![0u32; ch];
    let mut clip_runs = 0u64;
    let mut sums = vec![0f64; ch];
    let mut frames = 0u64;

    let mut stft = stft_size.map(|n| {
        let hop_s = (n / 4) as f64 / spec.sample_rate as f64;
        let duration = total as f64 / spec.sample_rate as f64;
        let snapshot_s = (duration / MINSTAT_MAX_SNAPSHOTS).max(0.5);
        (
            StftEngine::new(ch, n, false),
            FeatureExtractor::new(spec.sample_rate, n, ch),
            MinStatTracker::new(ch, n / 2 + 1, hop_s, MINSTAT_WINDOW_S, snapshot_s),
        )
    });

    src.seek(0).map_err(audio_err(path))?;
    loop {
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let n = src.read(&mut buf, BLOCK_FRAMES).map_err(audio_err(path))?;
        if n == 0 {
            break;
        }
        meter.add(&buf, n).map_err(analysis_err)?;
        for c in 0..ch {
            let mut s = 0f64;
            for &x in &buf[c][..n] {
                s += x as f64;
                if x.abs() >= CLIP_LEVEL {
                    run[c] += 1;
                    if run[c] == CLIP_RUN {
                        clip_runs += 1;
                    }
                } else {
                    run[c] = 0;
                }
            }
            sums[c] += s;
        }
        if let Some((eng, fx, ms)) = stft.as_mut() {
            eng.process(&buf, n, &mut [], |f| {
                fx.push(f.spectra, f.time);
                ms.push(f.spectra);
            });
        }
        frames += n as u64;
        progress.advance(id, Pass::Analyze, n as u64);
    }

    let (features, classes, minstat) = match stft {
        Some((mut eng, mut fx, mut ms)) => {
            // Nachlauf, damit auch die letzten Samples in vollständigen Frames liegen.
            let pad = vec![vec![0f32; eng.fft_size()]; ch];
            eng.process(&pad, eng.fft_size(), &mut [], |f| {
                fx.push(f.spectra, f.time);
                ms.push(f.spectra);
            });
            let features = fx.finish();
            let classes = classify(&features, &VadParams::default());
            (Some(features), classes, Some(ms.profile()))
        }
        None => (None, Vec::new(), None),
    };

    let dc_dbfs = sums
        .iter()
        .map(|&s| {
            let mean = (s / frames.max(1) as f64).abs();
            (mean > db_to_lin(-50.0)).then(|| 20.0 * mean.log10())
        })
        .collect();
    Ok(InputAnalysis {
        spec,
        frames,
        loudness: meter.stats().map_err(analysis_err)?,
        clip_runs,
        dc_dbfs,
        features,
        classes,
        minstat,
    })
}

fn input_info(a: &InputAnalysis) -> InputInfo {
    InputInfo {
        sample_rate: a.spec.sample_rate,
        channels: a.spec.channels,
        format: a.spec.format.label().to_string(),
        frames: a.frames,
        duration_s: a.frames as f64 / a.spec.sample_rate as f64,
    }
}

fn input_warnings(a: &InputAnalysis) -> Vec<String> {
    let mut w = Vec::new();
    if a.clip_runs > 0 {
        w.push(format!(
            "Eingabe enthält {} übersteuerte Stellen (≥ {CLIP_RUN} Samples an Vollaussteuerung); \
             Clipping ist nicht reparierbar",
            a.clip_runs
        ));
    }
    for (c, dc) in a.dc_dbfs.iter().enumerate() {
        if let Some(db) = dc {
            w.push(format!(
                "Kanal {}: Gleichspannungsanteil {db:.1} dBFS",
                c + 1
            ));
        }
    }
    w
}

fn base_report(
    path: &Path,
    a: &InputAnalysis,
    started: Instant,
    mut warnings: Vec<String>,
) -> FileReport {
    let elapsed = started.elapsed().as_secs_f64();
    let duration = a.frames as f64 / a.spec.sample_rate as f64;
    let mut all = input_warnings(a);
    all.append(&mut warnings);
    FileReport {
        input_path: path.to_path_buf(),
        input: input_info(a),
        loudness_in: a.loudness,
        input_clip_runs: a.clip_runs,
        dc_offset_dbfs: a.dc_dbfs.clone(),
        denoise: None,
        normalization_gain_db: None,
        limiter: None,
        output: None,
        loudness_out: None,
        warnings: all,
        elapsed_s: elapsed,
        realtime_factor: if elapsed > 0.0 {
            duration / elapsed
        } else {
            0.0
        },
    }
}

/// Pass 1 und Profilentscheidung, gemeinsam für analyze/process/learn.
fn analyze_and_plan(
    src: &mut dyn AudioSource,
    path: &Path,
    settings: &Settings,
    forced: Option<&NoiseProfile>,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<(InputAnalysis, Option<NoisePlan>)> {
    let d = &settings.denoise;
    let stft = d.enabled.then(|| fft_size_for(d, src.spec().sample_rate));
    let a = analysis_pass(src, path, stft, id, progress, cancel)?;
    let plan = if d.enabled {
        Some(plan_noise(src, path, &a, d, forced)?)
    } else {
        None
    };
    Ok((a, plan))
}

/// Nur messen und das Rauschprofil schätzen, nichts schreiben.
pub fn analyze_file(
    path: &Path,
    settings: &Settings,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<FileReport> {
    let started = Instant::now();
    let mut src = open(path)?;
    progress.file_started(id, src.total_frames(), 1);
    let (a, plan) = analyze_and_plan(src.as_mut(), path, settings, None, id, progress, cancel)?;
    progress.file_finished(id);
    let mut warnings = Vec::new();
    let denoise = plan.map(|p| {
        warnings.extend(p.warnings);
        p.report
    });
    let mut r = base_report(path, &a, started, warnings);
    r.denoise = denoise;
    Ok(r)
}

/// Rauschprofil aus einer Datei lernen (z. B. Raumton-Aufnahme).
pub fn learn_profile(
    path: &Path,
    settings: &Settings,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<(NoiseProfile, FileReport)> {
    let started = Instant::now();
    let mut s = settings.clone();
    s.denoise.enabled = true;
    let mut src = open(path)?;
    progress.file_started(0, src.total_frames(), 1);
    let (a, plan) = analyze_and_plan(src.as_mut(), path, &s, None, 0, progress, cancel)?;
    progress.file_finished(0);
    let plan = plan.expect("Rauschreduktion aktiv");
    let profile = plan.profile.ok_or_else(|| {
        PipelineError::Analysis(format!("{}: kein Rauschprofil bestimmbar", path.display()))
    })?;
    let mut r = base_report(path, &a, started, plan.warnings);
    r.denoise = Some(plan.report);
    Ok((profile, r))
}

/// Ausgabeformat aus Eingabeformat und Einstellung.
pub fn resolve_output_spec(input: AudioSpec, depth: BitDepth) -> AudioSpec {
    let format = match depth {
        BitDepth::Same => match input.format {
            SampleFormat::Int32 => SampleFormat::Int24,
            f => f,
        },
        BitDepth::I16 => SampleFormat::Int16,
        BitDepth::I24 => SampleFormat::Int24,
        BitDepth::F32 => SampleFormat::Float32,
    };
    AudioSpec { format, ..input }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn delta_path(output: &Path) -> PathBuf {
    let stem = output.file_stem().unwrap_or_default().to_string_lossy();
    output.with_file_name(format!("{stem}.delta.wav"))
}

/// Entfernt Teildateien, falls die Verarbeitung nicht erfolgreich endet.
struct PartialGuard {
    paths: Vec<PathBuf>,
    armed: bool,
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if self.armed {
            for p in &self.paths {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// Welcher Teil eines Blocks gehört zur Ausgabe eines Stroms mit `latency`?
/// Liefert (zu verwerfende Frames am Blockanfang, zu übernehmende Frames).
fn span(latency: u64, total: u64, fed_before: u64, n: usize) -> (usize, usize) {
    let start = fed_before as i64 - latency as i64;
    let skip = if start < 0 {
        ((-start) as usize).min(n)
    } else {
        0
    };
    let first = start.max(0) as u64;
    let take = ((n - skip) as u64).min(total.saturating_sub(first)) as usize;
    (skip, take)
}

/// Liest die Eingabe blockweise und speist danach Nullen nach, bis ein Strom
/// mit `latency` vollständig ausgegeben ist. `step(block, n, fed_before)`.
#[allow(clippy::too_many_arguments)]
fn drive<F>(
    src: &mut dyn AudioSource,
    path: &Path,
    total: u64,
    latency: u64,
    pass: Pass,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
    mut step: F,
) -> Result<()>
where
    F: FnMut(&mut Vec<Vec<f32>>, usize, u64) -> Result<()>,
{
    let ch = src.spec().channels as usize;
    let mut buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut fed = 0u64;
    src.seek(0).map_err(audio_err(path))?;
    while fed < total + latency {
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let mut n = src.read(&mut buf, BLOCK_FRAMES).map_err(audio_err(path))?;
        if n == 0 {
            n = (total + latency - fed).min(BLOCK_FRAMES as u64) as usize;
            for c in buf.iter_mut() {
                c.clear();
                c.resize(n, 0.0);
            }
        } else {
            progress.advance(id, pass, n as u64);
        }
        step(&mut buf, n, fed)?;
        fed += n as u64;
    }
    Ok(())
}

fn denoise_params(d: &DenoiseSettings) -> DenoiseParams {
    DenoiseParams {
        reduction_db: d.reduction_db,
        oversubtraction: d.oversubtraction,
        gain_rule: match d.gain_rule {
            GainRuleSetting::WienerDd => GainRule::WienerDd,
            GainRuleSetting::Subtract => GainRule::Subtract,
        },
        floor: d.floor,
        dd_alpha: 0.98,
        release_ms: d.release_ms,
        freq_smooth_oct: d.freq_smooth_oct,
        stereo_link: d.stereo_link,
    }
}

fn new_denoise_stage(a: &InputAnalysis, power: &[Vec<f32>], d: &DenoiseSettings) -> DenoiseStage {
    DenoiseStage::new(
        power,
        fft_size_for(d, a.spec.sample_rate),
        a.spec.sample_rate,
        denoise_params(d),
        a.classes.clone(),
    )
}

/// Pass 2: Lautheit nach der Rauschreduktion messen.
#[allow(clippy::too_many_arguments)]
fn measure_pass(
    src: &mut dyn AudioSource,
    path: &Path,
    a: &InputAnalysis,
    power: &[Vec<f32>],
    d: &DenoiseSettings,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<LoudnessStats> {
    let ch = a.spec.channels as usize;
    let mut stage = new_denoise_stage(a, power, d);
    let latency = stage.latency() as u64;
    let mut meter = LoudnessMeter::new(ch, a.spec.sample_rate).map_err(analysis_err)?;
    let mut out = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    drive(
        src,
        path,
        a.frames,
        latency,
        Pass::Measure,
        id,
        progress,
        cancel,
        |block, n, fed| {
            stage.process(block, n, &mut out);
            let (skip, take) = span(latency, a.frames, fed, n);
            if take > 0 {
                let view: Vec<Vec<f32>> =
                    out.iter().map(|c| c[skip..skip + take].to_vec()).collect();
                meter.add(&view, take).map_err(analysis_err)?;
            }
            Ok(())
        },
    )?;
    meter.stats().map_err(analysis_err)
}

/// Komplette Verarbeitung: analysieren, entrauschen, normalisieren,
/// limitieren, schreiben.
///
/// Geschrieben wird zuerst `<ausgabe>.partial`, erst nach Erfolg wird
/// umbenannt. Bei Fehler oder Abbruch werden Teildateien gelöscht.
pub fn process_file(
    input: &Path,
    output: &Path,
    settings: &Settings,
    opts: &JobOptions,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<FileReport> {
    let started = Instant::now();
    let mut src = open(input)?;
    let total = src.total_frames();
    let d = &settings.denoise;
    progress.file_started(id, total, if d.enabled { 3 } else { 2 });

    // ---- Pass 1: Analyse + Rauschprofil
    let (a, plan) = analyze_and_plan(
        src.as_mut(),
        input,
        settings,
        opts.noise_profile.as_deref(),
        id,
        progress,
        cancel,
    )?;
    let mut warnings = Vec::new();
    let mut denoise_report = None;
    let mut noise_power = None;
    if let Some(p) = plan {
        warnings.extend(p.warnings);
        let mut rep = p.report;
        if let (Some(dir), Some(prof)) = (&opts.save_profile_dir, &p.profile)
            && opts.noise_profile.is_none()
        {
            let stem = input.file_stem().unwrap_or_default().to_string_lossy();
            let path = dir.join(format!("{stem}.noise.json"));
            save_profile(&path, prof)?;
            rep.profile_saved = Some(path);
        }
        noise_power = p.power;
        denoise_report = Some(rep);
    }

    // ---- Pass 2: Lautheit nach Rauschreduktion
    let loudness_for_gain = match &noise_power {
        Some(power) => measure_pass(src.as_mut(), input, &a, power, d, id, progress, cancel)?,
        None => {
            // Ohne Rauschreduktion: Messen entfällt, Fortschritt trotzdem vollständig.
            if d.enabled {
                progress.advance(id, Pass::Measure, total);
            }
            a.loudness
        }
    };

    let gain_db = if settings.loudness.enabled {
        match loudness_for_gain.integrated_lufs {
            Some(i) => settings.loudness.target_lufs - i,
            None => {
                warnings.push(
                    "Eingabe ist (nahezu) stumm; Lautheit nicht messbar, kein Normalisierungs-Gain"
                        .into(),
                );
                0.0
            }
        }
    } else {
        0.0
    };
    if gain_db > 20.0 {
        warnings.push(format!(
            "sehr hohe Anhebung um {gain_db:.1} dB; Restrauschen wird entsprechend lauter"
        ));
    }

    // ---- Pass 3: Rendern
    let out_spec = resolve_output_spec(a.spec, settings.output.bit_depth);
    let ch = a.spec.channels as usize;
    let dither_on = match settings.output.dither {
        DitherMode::Auto => out_spec.format == SampleFormat::Int16,
        DitherMode::On => !out_spec.format.is_float(),
        DitherMode::Off => false,
    };
    let mut dither = dither_on.then(|| TpdfDither::new(ch, out_spec.format.bits()));
    let mut stage = noise_power.as_ref().map(|p| new_denoise_stage(&a, p, d));
    let mut limiter = settings.limiter.enabled.then(|| {
        TruePeakLimiter::new(
            ch,
            a.spec.sample_rate,
            LimiterParams {
                ceiling_dbtp: settings.limiter.ceiling_dbtp,
                lookahead_ms: settings.limiter.lookahead_ms,
                release_ms: settings.limiter.release_ms,
                oversampling: settings.limiter.oversampling,
            },
        )
    });
    let dn_latency = stage.as_ref().map_or(0, |s| s.latency()) as u64;
    let latency = dn_latency + limiter.as_ref().map_or(0, |l| l.latency()) as u64;

    let partial = with_suffix(output, ".partial");
    let delta_final = (opts.export_delta && stage.is_some()).then(|| delta_path(output));
    let delta_partial = delta_final.as_ref().map(|p| with_suffix(p, ".partial"));
    let mut guard = PartialGuard {
        paths: std::iter::once(partial.clone())
            .chain(delta_partial.clone())
            .collect(),
        armed: true,
    };
    let mut sink = leveled_audio_io::create_sink(&partial, out_spec).map_err(audio_err(output))?;
    let mut delta_sink: Option<Box<dyn AudioSink>> = match &delta_partial {
        Some(p) => Some(
            leveled_audio_io::create_sink(
                p,
                AudioSpec {
                    format: SampleFormat::Float32,
                    ..a.spec
                },
            )
            .map_err(audio_err(p))?,
        ),
        None => None,
    };
    let mut dry_delay = delta_sink
        .as_ref()
        .map(|_| DelayLine::new(ch, dn_latency as usize));
    let mut meter = LoudnessMeter::new(ch, a.spec.sample_rate).map_err(analysis_err)?;

    let gain = db_to_lin(gain_db) as f32;
    let mut den_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut lim_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut dry_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    drive(
        src.as_mut(),
        input,
        a.frames,
        latency,
        Pass::Render,
        id,
        progress,
        cancel,
        |block, n, fed| {
            if let Some(dl) = dry_delay.as_mut() {
                dl.process(block, n, &mut dry_buf);
            }
            let cur: &mut Vec<Vec<f32>> = match stage.as_mut() {
                Some(s) => {
                    s.process(block, n, &mut den_buf);
                    &mut den_buf
                }
                None => block,
            };
            if let Some(ds) = delta_sink.as_mut() {
                let (skip, take) = span(dn_latency, a.frames, fed, n);
                if take > 0 {
                    let delta: Vec<Vec<f32>> = (0..ch)
                        .map(|c| {
                            (skip..skip + take)
                                .map(|i| dry_buf[c][i] - cur[c][i])
                                .collect()
                        })
                        .collect();
                    ds.write(&delta, take).map_err(audio_err(output))?;
                }
            }
            apply_gain(cur, n, gain);
            let out: &mut Vec<Vec<f32>> = match limiter.as_mut() {
                Some(l) => {
                    l.process(cur, &mut lim_buf, n);
                    &mut lim_buf
                }
                None => cur,
            };
            let (skip, take) = span(latency, a.frames, fed, n);
            if take == 0 {
                return Ok(());
            }
            if skip > 0 {
                for c in out.iter_mut() {
                    c.drain(..skip);
                }
            }
            if let Some(dt) = dither.as_mut() {
                dt.apply(out, take);
            }
            sink.write(out, take).map_err(audio_err(output))?;
            meter.add(out, take).map_err(analysis_err)?;
            Ok(())
        },
    )?;

    let clipped = sink.clipped_samples();
    sink.finalize().map_err(audio_err(output))?;
    let rename = |from: &Path, to: &Path| {
        std::fs::rename(from, to).map_err(|e| PipelineError::Io {
            path: to.to_path_buf(),
            message: e.to_string(),
        })
    };
    if let (Some(ds), Some(dp), Some(df)) = (delta_sink, &delta_partial, &delta_final) {
        ds.finalize().map_err(audio_err(df))?;
        rename(dp, df)?;
    }
    rename(&partial, output)?;
    guard.armed = false;
    progress.file_finished(id);

    if clipped > 0 {
        warnings.push(format!("{clipped} Samples beim Quantisieren begrenzt"));
    }
    let loudness_out = meter.stats().map_err(analysis_err)?;
    if let (true, Some(i)) = (settings.loudness.enabled, loudness_out.integrated_lufs) {
        let dev = i - settings.loudness.target_lufs;
        if dev.abs() > 0.5 {
            warnings.push(format!(
                "Ziel-Lautheit um {dev:+.1} LU verfehlt (Limiter greift stark ein)"
            ));
        }
    }

    if let (Some(rep), Some(st)) = (denoise_report.as_mut(), stage.as_ref()) {
        let (pin, pout) = st.pause_levels_db();
        rep.pause_level_in_dbfs = pin;
        rep.pause_level_out_dbfs = pout;
        rep.attenuation_pauses_db = st.stats.attenuation_pauses_db();
        rep.attenuation_speech_db = st.stats.attenuation_speech_db();
        rep.delta_path = delta_final.clone();
    }

    let mut report = base_report(input, &a, started, warnings);
    report.denoise = denoise_report;
    report.normalization_gain_db = Some(gain_db);
    report.limiter = limiter.map(|l| {
        let s = l.stats();
        LimiterReport {
            max_gain_reduction_db: s.max_reduction_db(),
            limited_fraction: s.limited_fraction(),
        }
    });
    report.output = Some(OutputInfo {
        path: output.to_path_buf(),
        format: out_spec.format.label().to_string(),
        clipped_samples: clipped,
    });
    report.loudness_out = Some(loudness_out);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::span;

    #[test]
    fn span_alignment() {
        // Latenz 10, Gesamtlänge 25, Blöcke à 8.
        assert_eq!(span(10, 25, 0, 8), (8, 0));
        assert_eq!(span(10, 25, 8, 8), (2, 6));
        assert_eq!(span(10, 25, 16, 8), (0, 8));
        assert_eq!(span(10, 25, 32, 8), (0, 3));
        assert_eq!(span(0, 5, 0, 8), (0, 5));
    }
}
