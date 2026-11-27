//! Verarbeitung einer einzelnen Datei.
//!
//! Passes (jeweils streamend, konstanter Speicher):
//! 1. **Analyse**: Lautheit, Clipping, DC; mit Rauschreduktion oder Leveler
//!    zusätzlich STFT-Features, VAD und Minimum Statistics.
//!    Danach offline: Rauschprofil bestimmen (gezieltes Nachlesen der Pausen)
//!    und die Leveler-Kurve berechnen.
//! 2. **Messen** (nur mit Rauschreduktion/Leveler): Lautheit nach beiden.
//! 3. **Rendern**: Entrauschen → Leveler → Gain → True-Peak-Limiter → Dither → Datei.

use crate::batch::run_batch;
use crate::denoise_stage::DenoiseStage;
use crate::leveler_stage::LevelerStage;
use crate::noise::{NoisePlan, plan_noise, save_profile};
use crate::progress::{CancelToken, Pass, ProgressSink};
use crate::report::{
    AlbumReport, CompressorReport, DenoiseReport, FileReport, GateReport, InputInfo, KnockReport,
    LimiterReport, OutputInfo,
};
use crate::settings::{
    BitDepth, DenoiseSettings, DitherMode, GainRuleSetting, HighpassSettings, LevelerSettings,
    Settings,
};
use crate::stages::{FilteredSource, gate_mask, knock_bin_weights, knock_plan};
use crate::{BLOCK_FRAMES, PipelineError, Result};
use leveled_audio_analysis::{
    FeatureExtractor, FrameClass, FrameFeatures, LevelerCurve, LevelerParams, LoudnessMeter,
    LoudnessStats, MinStatTracker, NoiseProfile, VadParams, classify, k_weighted_power,
    leveler_curve,
};
use leveled_audio_dsp::{
    Compressor, CompressorParams, CompressorStats, DelayLine, DenoiseParams, GainRule, Highpass,
    LimiterParams, StftEngine, TpdfDither, TruePeakLimiter, apply_gain, db_to_lin,
};
use leveled_audio_io::{AudioSink, AudioSource, AudioSpec, SampleFormat};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Messung aus Pass 1 (für den Album-Modus ohne Pass 2).
    pub meter: Option<LoudnessMeter>,
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
#[allow(clippy::too_many_arguments)]
fn analysis_pass(
    src: &mut dyn AudioSource,
    path: &Path,
    stft_size: Option<usize>,
    highpass: &HighpassSettings,
    vad: &VadParams,
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
    // Lautheit, Clipping und DC werden am Original gemessen; die STFT-Analyse
    // sieht das Signal nach dem Hochpass (wie alle folgenden Stufen).
    let mut hp = (stft.is_some() && highpass.enabled)
        .then(|| Highpass::new(ch, spec.sample_rate, highpass.frequency_hz));
    let mut filtered = vec![Vec::with_capacity(BLOCK_FRAMES); ch];

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
            let input: &Vec<Vec<f32>> = match hp.as_mut() {
                Some(h) => {
                    for (f, b) in filtered.iter_mut().zip(&buf) {
                        f.clear();
                        f.extend_from_slice(&b[..n]);
                    }
                    h.process(&mut filtered, n);
                    &filtered
                }
                None => &buf,
            };
            eng.process(input, n, &mut [], |f| {
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
            let classes = classify(&features, vad);
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
        meter: Some(meter),
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
    elapsed: Duration,
    mut warnings: Vec<String>,
) -> FileReport {
    let elapsed = elapsed.as_secs_f64();
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
        highpass_hz: None,
        knock: None,
        leveler: None,
        gate: None,
        compressor: None,
        normalization_gain_db: None,
        loudness_correction_db: None,
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

/// Braucht die Einstellung die STFT-Analyse (VAD, Rauschprofil)?
fn needs_analysis(settings: &Settings) -> bool {
    settings.denoise.enabled
        || settings.leveler.active()
        || settings.compressor.active()
        || settings.gate.enabled
}

/// Braucht die Einstellung die Sprach-Referenz bzw. Leveler-Kurve?
fn needs_curve(settings: &Settings) -> bool {
    settings.leveler.active() || settings.compressor.active() || settings.gate.enabled
}

fn vad_params(settings: &Settings) -> VadParams {
    VadParams {
        noise_max_speech_to_low_db: settings
            .knock
            .enabled
            .then_some(settings.knock.max_speech_to_low_db as f32),
        ..VadParams::default()
    }
}

fn leveler_params(l: &LevelerSettings) -> LevelerParams {
    LevelerParams {
        strength: l.strength,
        max_boost_db: l.max_boost_db,
        max_cut_db: l.max_cut_db,
        pause_max_boost_db: l.pause_max_boost_db,
        slew_db_per_s: l.slew_db_per_s,
        ..Default::default()
    }
}

/// Ergebnis von Pass 1 samt offline abgeleiteter Entscheidungen.
struct Planned {
    a: InputAnalysis,
    /// Rauschprofil (auch ohne Rauschreduktion, zur Kompensation im Leveler).
    noise: Option<NoisePlan>,
    /// Leveler-Kurve (flach, wenn der Leveler aus ist) samt Sprach-Referenz;
    /// vorhanden, wenn Leveler, Kompressor oder Pausen-Absenkung sie brauchen.
    leveler: Option<LevelerCurve>,
}

/// Pass 1, Profilentscheidung und Leveler-Kurve, gemeinsam für analyze/process/learn.
fn analyze_and_plan(
    src: &mut dyn AudioSource,
    path: &Path,
    settings: &Settings,
    forced: Option<&NoiseProfile>,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<Planned> {
    let d = &settings.denoise;
    let fft = fft_size_for(d, src.spec().sample_rate);
    let stft = needs_analysis(settings).then_some(fft);
    let a = analysis_pass(
        src,
        path,
        stft,
        &settings.highpass,
        &vad_params(settings),
        id,
        progress,
        cancel,
    )?;
    let noise = if stft.is_some() {
        let mut fsrc = FilteredSource::new(src, &settings.highpass);
        Some(plan_noise(&mut fsrc, path, &a, d, forced)?)
    } else {
        None
    };
    let leveler = match (&a.features, needs_curve(settings)) {
        (Some(f), true) => {
            let mut lp = leveler_params(&settings.leveler);
            if !settings.leveler.active() {
                lp.strength = 0.0;
            }
            // Rauschleistung abziehen, damit leise Phrasen nicht zu laut geschätzt werden.
            let noise_k = noise
                .as_ref()
                .and_then(|p| p.power.as_ref())
                .map_or(0.0, |p| k_weighted_power(p, a.spec.sample_rate, fft));
            Some(leveler_curve(f, &a.classes, noise_k, &lp))
        }
        _ => None,
    };
    Ok(Planned { a, noise, leveler })
}

fn leveler_warnings(curve: &LevelerCurve) -> Vec<String> {
    if curve.stats.phrases == 0 {
        vec!["keine Sprache erkannt; Leveler ohne Wirkung".into()]
    } else {
        Vec::new()
    }
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
    let Planned { a, noise, leveler } =
        analyze_and_plan(src.as_mut(), path, settings, None, id, progress, cancel)?;
    progress.file_finished(id);
    let mut warnings = Vec::new();
    let denoise = noise.filter(|_| settings.denoise.enabled).map(|p| {
        warnings.extend(p.warnings);
        p.report
    });
    let leveler = leveler.filter(|_| settings.leveler.active());
    if let Some(c) = &leveler {
        warnings.extend(leveler_warnings(c));
    }
    let mut r = base_report(path, &a, started.elapsed(), warnings);
    r.denoise = denoise;
    r.leveler = leveler.map(|c| c.stats);
    r.knock = knock_report(&a, settings);
    r.highpass_hz = settings
        .highpass
        .enabled
        .then_some(settings.highpass.frequency_hz);
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
    let Planned { a, noise, .. } =
        analyze_and_plan(src.as_mut(), path, &s, None, 0, progress, cancel)?;
    progress.file_finished(0);
    let plan = noise.expect("Rauschreduktion aktiv");
    let profile = plan.profile.ok_or_else(|| {
        PipelineError::Analysis(format!("{}: kein Rauschprofil bestimmbar", path.display()))
    })?;
    let mut r = base_report(path, &a, started.elapsed(), plan.warnings);
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

fn new_denoise_stage(
    a: &InputAnalysis,
    power: &[Vec<f32>],
    d: &DenoiseSettings,
    knock: Option<&[f32]>,
) -> DenoiseStage {
    let fft = fft_size_for(d, a.spec.sample_rate);
    let stage = DenoiseStage::new(
        power,
        fft,
        a.spec.sample_rate,
        denoise_params(d),
        a.classes.clone(),
    );
    match knock {
        Some(k) => stage.with_knock(k, knock_bin_weights(a.spec.sample_rate, fft)),
        None => stage,
    }
}

/// Stufen zwischen Rauschreduktion und Normalisierungs-Gain, gemeinsam für
/// Pass 2 und 3 (beide müssen dasselbe rechnen).
struct PreGain<'a> {
    a: &'a InputAnalysis,
    noise_power: Option<&'a [Vec<f32>]>,
    knock: Option<&'a [f32]>,
    d: &'a DenoiseSettings,
    curve: Option<&'a LevelerStage>,
    compressor: Option<CompressorParams>,
}

impl PreGain<'_> {
    fn denoise_stage(&self) -> Option<DenoiseStage> {
        self.noise_power
            .map(|p| new_denoise_stage(self.a, p, self.d, self.knock))
    }

    fn compressor(&self) -> Option<Compressor> {
        self.compressor
            .map(|p| Compressor::new(self.a.spec.channels as usize, self.a.spec.sample_rate, p))
    }
}

/// Pausen-Messung in Pass 2: Leistung der Ausgabe in Frames mit voller Maske,
/// zusätzlich ohne den Leveler-Gain (damit die Absenkung je Pause den
/// Leveler ausgleichen kann).
struct PauseMeter<'a> {
    mask: &'a [f32],
    /// Leveler-Kurve, falls der Leveler wirkt.
    leveler: Option<&'a leveled_audio_analysis::LevelerCurve>,
    first_center: i64,
    hop: usize,
    sum: f64,
    sum_pre: f64,
    count: u64,
}

/// Gemessener Pausenpegel vor dem Normalisierungs-Gain (dBFS).
#[derive(Debug, Clone, Copy)]
struct PauseLevel {
    /// Mit Leveler (für den Report).
    out_db: f64,
    /// Ohne Leveler-Gain (für die Tiefe je Frame).
    pre_db: f64,
}

impl PauseMeter<'_> {
    fn add(&mut self, out: &[Vec<f32>], from: usize, take: usize, first_sample: i64) {
        let ch = out.len() as f64;
        for i in 0..take {
            let t = first_sample + i as i64;
            let k = ((t - self.first_center) as f64 / self.hop as f64).round();
            let k = (k.max(0.0) as usize).min(self.mask.len().saturating_sub(1));
            if self.mask.get(k).is_some_and(|m| *m >= 0.999) {
                let p = out
                    .iter()
                    .map(|c| (c[from + i] as f64).powi(2))
                    .sum::<f64>()
                    / ch;
                let lev = self.leveler.map_or(0.0, |c| c.gain_db_at(t) as f64);
                self.sum += p;
                self.sum_pre += p * 10f64.powf(-lev / 10.0);
                self.count += 1;
            }
        }
    }

    fn level(&self) -> Option<PauseLevel> {
        let db = |s: f64| 10.0 * (s / self.count as f64).log10();
        (self.count > 0 && self.sum > 0.0).then(|| PauseLevel {
            out_db: db(self.sum),
            pre_db: db(self.sum_pre),
        })
    }
}

/// Pass 2: Lautheit nach Rauschreduktion und Leveler messen.
#[allow(clippy::too_many_arguments)]
fn measure_pass(
    src: &mut dyn AudioSource,
    path: &Path,
    chain: &PreGain,
    mut pause: Option<PauseMeter>,
    probe: Option<(f64, LimiterParams)>,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<MeasureOut> {
    let a = chain.a;
    let ch = a.spec.channels as usize;
    let mut stage = chain.denoise_stage();
    let mut comp = chain.compressor();
    let dn_latency = stage.as_ref().map_or(0, |s| s.latency()) as u64;
    let latency = dn_latency + comp.as_ref().map_or(0, |c| c.latency()) as u64;
    let leveler = chain.curve;
    let mut meter = LoudnessMeter::new(ch, a.spec.sample_rate).map_err(analysis_err)?;
    let mut out = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut comp_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    // Limiter-Probe: geschätzter Gain + Limiter, um den Lautheitsverlust durch
    // den Limiter schon vor dem Rendern zu kennen (spart meist eine
    // Korrekturiteration).
    let mut probe = match probe {
        Some((g, p)) => Some((
            db_to_lin(g) as f32,
            TruePeakLimiter::new(ch, a.spec.sample_rate, p),
            LoudnessMeter::new(ch, a.spec.sample_rate).map_err(analysis_err)?,
            vec![Vec::with_capacity(BLOCK_FRAMES); ch],
            vec![Vec::with_capacity(BLOCK_FRAMES); ch],
        )),
        None => None,
    };
    let probe_latency = probe.as_ref().map_or(0, |p| p.1.latency()) as u64;
    drive(
        src,
        path,
        a.frames,
        latency + probe_latency,
        Pass::Measure,
        id,
        progress,
        cancel,
        |block, n, fed| {
            let cur: &mut Vec<Vec<f32>> = match stage.as_mut() {
                Some(s) => {
                    s.process(block, n, &mut out);
                    &mut out
                }
                None => block,
            };
            if let Some(l) = leveler {
                l.apply(cur, n, fed as i64 - dn_latency as i64);
            }
            let out: &Vec<Vec<f32>> = match comp.as_mut() {
                Some(c) => {
                    c.process(cur, &mut comp_buf, n);
                    &comp_buf
                }
                None => cur,
            };
            let (skip, take) = span(latency, a.frames, fed, n);
            if take > 0 {
                let view: Vec<Vec<f32>> =
                    out.iter().map(|c| c[skip..skip + take].to_vec()).collect();
                meter.add(&view, take).map_err(analysis_err)?;
                if let Some(p) = pause.as_mut() {
                    p.add(out, skip, take, fed as i64 + skip as i64 - latency as i64);
                }
            }
            if let Some((g, lim, m, gained, limited)) = probe.as_mut() {
                for (dst, src) in gained.iter_mut().zip(out.iter()) {
                    dst.clear();
                    dst.extend(src[..n].iter().map(|v| v * *g));
                }
                lim.process(gained, limited, n);
                let (skip, take) = span(latency + probe_latency, a.frames, fed, n);
                if take > 0 {
                    let view: Vec<Vec<f32>> = limited
                        .iter()
                        .map(|c| c[skip..skip + take].to_vec())
                        .collect();
                    m.add(&view, take).map_err(analysis_err)?;
                }
            }
            Ok(())
        },
    )?;
    let probe = match probe {
        Some((_, _, m, _, _)) => m.stats().map_err(analysis_err)?.integrated_lufs,
        None => None,
    };
    Ok(MeasureOut {
        meter,
        pause_db: pause.and_then(|p| p.level()),
        probe_lufs: probe,
    })
}

struct MeasureOut {
    meter: LoudnessMeter,
    pause_db: Option<PauseLevel>,
    /// Lautheit nach geschätztem Gain und Limiter (Limiter-Probe).
    probe_lufs: Option<f64>,
}

fn limiter_params(settings: &Settings) -> LimiterParams {
    LimiterParams {
        ceiling_dbtp: settings.limiter.ceiling_dbtp,
        lookahead_ms: settings.limiter.lookahead_ms,
        release_ms: settings.limiter.release_ms,
        oversampling: settings.limiter.oversampling,
    }
}

/// Ab dieser Abweichung der Ausgangslautheit vom Ziel folgt eine Korrekturiteration.
const CORRECTION_THRESHOLD_LU: f64 = 0.3;
/// Höchstens so viele Korrekturiterationen (die zweite nur, wenn die erste
/// nicht reicht; im Normalfall gar keine).
const MAX_CORRECTIONS: usize = 2;
/// Größte Gain-Änderung durch alle Korrekturiterationen zusammen.
const CORRECTION_MAX_DB: f64 = 6.0;

/// Sucht den Gain, bei dem die Ausgabe trotz Limiter das Ziel trifft.
///
/// Der Limiter macht den Zusammenhang Gain → Lautheit flacher als 1:1. Der
/// erste Schritt nimmt Steigung 1 an, der zweite schätzt sie aus den beiden
/// Messungen (Sekante), begrenzt auf 1 … 4 dB Gain je LU.
struct Corrector {
    target: f64,
    first_gain: f64,
    points: Vec<(f64, f64)>,
}

impl Corrector {
    fn new(target: f64, gain_db: f64) -> Self {
        Corrector {
            target,
            first_gain: gain_db,
            points: Vec::new(),
        }
    }

    /// Messung zum zuletzt verwendeten Gain eintragen; liefert den nächsten
    /// Gain oder `None`, wenn das Ziel getroffen ist oder keine Iteration bleibt.
    fn next(&mut self, gain_db: f64, lufs: f64) -> Option<f64> {
        self.points.push((gain_db, lufs));
        let dev = self.target - lufs;
        if dev.abs() <= CORRECTION_THRESHOLD_LU || self.points.len() > MAX_CORRECTIONS {
            return None;
        }
        let slope = match self.points[..] {
            [.., (g0, l0), (g1, l1)] if (l1 - l0).abs() > 1e-3 => {
                ((g1 - g0) / (l1 - l0)).clamp(1.0, 4.0)
            }
            _ => 1.0,
        };
        let next = (gain_db + slope * dev).clamp(
            self.first_gain - CORRECTION_MAX_DB,
            self.first_gain + CORRECTION_MAX_DB,
        );
        ((next - gain_db).abs() > 1e-3).then_some(next)
    }

    /// Stützpunkt ohne eigenen Render-Durchlauf (Limiter-Probe aus Pass 2).
    /// Liefert den Gain für den ersten Durchlauf: ein Schritt mit Steigung 1,
    /// begrenzt wie die Korrektur.
    fn seed(&mut self, gain_db: f64, lufs: f64) -> f64 {
        self.points.push((gain_db, lufs));
        (gain_db + self.target - lufs).clamp(
            self.first_gain - CORRECTION_MAX_DB,
            self.first_gain + CORRECTION_MAX_DB,
        )
    }

    /// Gesamte Korrektur gegenüber dem ersten Gain, falls iteriert wurde.
    fn correction(&self, final_gain: f64) -> Option<f64> {
        (self.points.len() > 1).then_some(final_gain - self.first_gain)
    }
}

/// Ergebnis von Analyse und Messung (Pass 1 und 2) einer Datei: alles, was
/// das Rendern braucht. Die Feature-Arrays sind bereits verworfen; übrig
/// bleiben VAD-Klassen, Rauschprofil und Leveler-Kurve (≈ 9 Byte je
/// STFT-Frame, rund 10 MB je Stunde Audio bei 48 kHz).
struct PreparedFile {
    input: PathBuf,
    /// Bisherige Rechenzeit (ohne Wartezeit an der Album-Barriere).
    busy: Duration,
    a: InputAnalysis,
    noise_power: Option<Vec<Vec<f32>>>,
    denoise_report: Option<DenoiseReport>,
    /// Störgeräusch-Absenkung je Frame (dB), falls aktiv.
    knock_gain: Option<Vec<f32>>,
    /// Leveler-Kurve (flach, wenn der Leveler aus ist); Grundlage der
    /// Gain-Stufe vor der Normalisierung.
    curve: Option<LevelerCurve>,
    leveler_active: bool,
    /// Pausen-Absenkung: Maske je Frame und Pausenpegel aus Pass 2 (vor Gain).
    gate: Option<(Vec<f32>, Option<PauseLevel>)>,
    compressor: Option<CompressorParams>,
    warnings: Vec<String>,
    /// Messung nach allen Stufen vor dem Normalisierungs-Gain.
    measured: LoudnessMeter,
    /// Limiter-Probe aus Pass 2: (geschätzter Gain, Lautheit danach).
    probe: Option<(f64, f64)>,
}

impl PreparedFile {
    /// Integrierte Lautheit vor dem Normalisierungs-Gain.
    fn loudness_lufs(&self) -> Option<f64> {
        self.measured.stats().ok().and_then(|s| s.integrated_lufs)
    }

    /// Mittlere Tiefe der Pausen-Absenkung für einen Normalisierungs-Gain
    /// (Report; gerendert wird je Frame, siehe `render_curve`).
    fn gate_depth(&self, settings: &Settings, gain_db: f64) -> Option<f64> {
        let (_, pause) = self.gate.as_ref()?;
        let g = &settings.gate;
        Some(((*pause)?.out_db + gain_db - g.floor_dbfs).clamp(0.0, g.max_attenuation_db))
    }

    /// Gain-Stufe für Pass 3: Leveler × Pausen-Absenkung.
    fn render_curve(&self, settings: &Settings, gain_db: f64) -> Option<LevelerStage> {
        let depth = self.gate_depth(settings, gain_db).filter(|d| *d > 0.0);
        if !self.leveler_active && depth.is_none() {
            return None;
        }
        let mut c = self.curve.clone()?;
        if !self.leveler_active {
            c.gain_db.iter_mut().for_each(|g| *g = 0.0);
        }
        // Tiefe je Frame: Der Leveler lässt Pausen nach angehobenen bzw.
        // abgesenkten Phrasen unterschiedlich laut stehen; jede soll auf dem
        // Ziel landen. Ohne Leveler ist das die einheitliche Tiefe.
        if let (Some(_), Some((mask, Some(p)))) = (depth, &self.gate) {
            let g = &settings.gate;
            let lim = g.max_attenuation_db as f32;
            let base = (p.pre_db + gain_db - g.floor_dbfs) as f32;
            for (v, m) in c.gain_db.iter_mut().zip(mask) {
                *v -= (base + *v).clamp(0.0, lim) * m;
            }
        }
        Some(LevelerStage::new(c))
    }
}

/// Wie viele Durchgänge `prepare` braucht (für die Fortschrittsanzeige).
fn prepare_passes(settings: &Settings) -> u32 {
    if needs_analysis(settings) || settings.highpass.enabled {
        2
    } else {
        1
    }
}

/// Störgeräusch-Statistik aus den VAD-Klassen.
fn knock_report(a: &InputAnalysis, settings: &Settings) -> Option<KnockReport> {
    if !settings.knock.enabled || a.classes.is_empty() {
        return None;
    }
    let hop_s = fft_size_for(&settings.denoise, a.spec.sample_rate) as f64
        / 4.0
        / a.spec.sample_rate as f64;
    let k = knock_plan(&a.classes, 0.0, 1);
    Some(KnockReport {
        regions: k.regions,
        duration_s: k.frames as f64 * hop_s,
        reduction_db: if settings.denoise.enabled {
            settings.knock.reduction_db
        } else {
            0.0
        },
    })
}

/// Pass 1 und 2 ohne Fortschrittsrahmen.
fn prepare(
    input: &Path,
    settings: &Settings,
    opts: &JobOptions,
    probe: bool,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<PreparedFile> {
    let started = Instant::now();
    let mut src = open(input)?;
    let total = src.total_frames();
    let d = &settings.denoise;

    // ---- Pass 1: Analyse + Rauschprofil + Leveler-Kurve
    let Planned {
        mut a,
        noise,
        leveler: curve,
    } = analyze_and_plan(
        src.as_mut(),
        input,
        settings,
        opts.noise_profile.as_deref(),
        id,
        progress,
        cancel,
    )?;
    let hop_s = a.features.as_ref().map_or(0.0, |f| f.hop_seconds());
    // Für das Rendern nicht mehr nötig; spart im Album-Modus Speicher.
    a.features = None;
    a.minstat = None;
    let mut warnings = Vec::new();
    let mut denoise_report = None;
    let mut noise_power = None;
    if let Some(p) = noise.filter(|_| d.enabled) {
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
    let leveler_active = settings.leveler.active() && curve.is_some();
    if let (true, Some(c)) = (leveler_active, &curve) {
        warnings.extend(leveler_warnings(c));
    }
    // Störgeräusch-Absenkung (nur zusammen mit der Rauschreduktion, PLAN 3.13).
    let knock_gain =
        (settings.knock.enabled && settings.knock.reduction_db > 0.0 && noise_power.is_some())
            .then(|| {
                knock_plan(
                    &a.classes,
                    settings.knock.reduction_db,
                    (0.03 / hop_s).round() as usize,
                )
                .gain_db
            })
            .filter(|g| g.iter().any(|v| *v < 0.0));
    // Kompressor: Schwelle relativ zum Sprach-Referenzpegel.
    let reference = curve.as_ref().and_then(|c| c.stats.reference_db);
    let c = &settings.compressor;
    let compressor = match (c.active(), reference) {
        (true, Some(r)) => Some(CompressorParams {
            threshold_dbfs: r + c.threshold_above_ref_db,
            ratio: Compressor::ratio_for_amount(c.amount),
            attack_ms: c.attack_ms,
            release_ms: c.release_ms,
            ..CompressorParams::default()
        }),
        (true, None) => {
            warnings.push("keine Sprache erkannt; Kompressor ohne Wirkung".into());
            None
        }
        _ => None,
    };
    let gate_mask = (settings.gate.enabled && !a.classes.is_empty() && hop_s > 0.0).then(|| {
        gate_mask(
            &a.classes,
            hop_s,
            &settings.gate,
            vad_params(settings).hangover_ms,
        )
    });

    // ---- Pass 2: Lautheit (und Pausenpegel) vor dem Normalisierungs-Gain
    let any_stage = noise_power.is_some()
        || leveler_active
        || compressor.is_some()
        || gate_mask.is_some()
        || settings.highpass.enabled;
    let mut probe_point = None;
    let (measured, gate) = if any_stage {
        let level_stage = curve
            .as_ref()
            .filter(|_| leveler_active)
            .map(|c| LevelerStage::new(c.clone()));
        let chain = PreGain {
            a: &a,
            noise_power: noise_power.as_deref(),
            knock: knock_gain.as_deref(),
            d,
            curve: level_stage.as_ref(),
            compressor,
        };
        let pause = match (&gate_mask, &curve) {
            (Some(m), Some(c)) => Some(PauseMeter {
                mask: m,
                leveler: leveler_active.then_some(c),
                first_center: c.first_center,
                hop: c.hop,
                sum: 0.0,
                sum_pre: 0.0,
                count: 0,
            }),
            _ => None,
        };
        // Gain-Schätzung für die Limiter-Probe aus der Eingangslautheit.
        let probe_gain = (probe && settings.loudness.enabled && settings.limiter.enabled)
            .then_some(a.loudness.integrated_lufs)
            .flatten()
            .map(|i| settings.loudness.target_lufs - i);
        let mut fsrc = FilteredSource::new(src.as_mut(), &settings.highpass);
        let m = measure_pass(
            &mut fsrc,
            input,
            &chain,
            pause,
            probe_gain.map(|g| (g, limiter_params(settings))),
            id,
            progress,
            cancel,
        )?;
        let gate = gate_mask.map(|mask| {
            if m.pause_db.is_none() {
                warnings.push("Pausen-Absenkung: keine ausreichend langen Pausen gefunden".into());
            }
            (mask, m.pause_db)
        });
        probe_point = probe_gain.zip(m.probe_lufs);
        (m.meter, gate)
    } else {
        // Ohne Bearbeitung vor dem Gain gilt die Messung aus Pass 1.
        if needs_analysis(settings) {
            progress.advance(id, Pass::Measure, total);
        }
        (a.meter.take().expect("Messung aus Pass 1"), None)
    };
    a.meter = None;
    Ok(PreparedFile {
        input: input.to_path_buf(),
        busy: started.elapsed(),
        a,
        noise_power,
        denoise_report,
        knock_gain,
        curve,
        leveler_active,
        gate,
        compressor,
        warnings,
        measured,
        probe: probe_point,
    })
}

/// Normalisierungs-Gain aus gemessener Lautheit (mit Warnungen).
fn normalization_gain(settings: &Settings, lufs: Option<f64>, warnings: &mut Vec<String>) -> f64 {
    if !settings.loudness.enabled {
        return 0.0;
    }
    let Some(i) = lufs else {
        warnings.push(
            "Eingabe ist (nahezu) stumm; Lautheit nicht messbar, kein Normalisierungs-Gain".into(),
        );
        return 0.0;
    };
    let gain_db = settings.loudness.target_lufs - i;
    if gain_db > 20.0 {
        warnings.push(format!(
            "sehr hohe Anhebung um {gain_db:.1} dB; Restrauschen wird entsprechend lauter"
        ));
    }
    gain_db
}

/// Wirkung der Rauschreduktion, gemessen beim Rendern.
struct DenoiseOut {
    pause_in: Option<f64>,
    pause_out: Option<f64>,
    att_pauses: Option<f64>,
    att_speech: Option<f64>,
}

struct RenderOut {
    meter: LoudnessMeter,
    limiter: Option<LimiterReport>,
    clipped: u64,
    denoise: Option<DenoiseOut>,
    compressor: Option<CompressorStats>,
    out_format: SampleFormat,
    delta: Option<PathBuf>,
}

/// Pass 3: rendert mit festem Gain nach `<ausgabe>.partial` und benennt erst
/// nach Erfolg um. Bei Fehler oder Abbruch werden Teildateien gelöscht.
#[allow(clippy::too_many_arguments)]
fn render_pass(
    prep: &PreparedFile,
    output: &Path,
    settings: &Settings,
    with_delta: bool,
    gain_db: f64,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<RenderOut> {
    let a = &prep.a;
    let d = &settings.denoise;
    let input = prep.input.as_path();
    let mut raw = open(input)?;
    let mut src = FilteredSource::new(raw.as_mut(), &settings.highpass);
    let out_spec = resolve_output_spec(a.spec, settings.output.bit_depth);
    let ch = a.spec.channels as usize;
    let dither_on = match settings.output.dither {
        DitherMode::Auto => out_spec.format == SampleFormat::Int16,
        DitherMode::On => !out_spec.format.is_float(),
        DitherMode::Off => false,
    };
    let mut dither = dither_on.then(|| TpdfDither::new(ch, out_spec.format.bits()));
    let curve = prep.render_curve(settings, gain_db);
    let chain = PreGain {
        a,
        noise_power: prep.noise_power.as_deref(),
        knock: prep.knock_gain.as_deref(),
        d,
        curve: curve.as_ref(),
        compressor: prep.compressor,
    };
    let mut stage = chain.denoise_stage();
    let mut comp = chain.compressor();
    let mut limiter = settings
        .limiter
        .enabled
        .then(|| TruePeakLimiter::new(ch, a.spec.sample_rate, limiter_params(settings)));
    let dn_latency = stage.as_ref().map_or(0, |s| s.latency()) as u64;
    let latency = dn_latency
        + comp.as_ref().map_or(0, |c| c.latency()) as u64
        + limiter.as_ref().map_or(0, |l| l.latency()) as u64;

    let partial = with_suffix(output, ".partial");
    let delta_final = (with_delta && stage.is_some()).then(|| delta_path(output));
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
    let leveler = curve.as_ref();
    let mut den_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut comp_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut lim_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut dry_buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    drive(
        &mut src,
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
            if let Some(l) = leveler {
                l.apply(cur, n, fed as i64 - dn_latency as i64);
            }
            let cur: &mut Vec<Vec<f32>> = match comp.as_mut() {
                Some(c) => {
                    c.process(cur, &mut comp_buf, n);
                    &mut comp_buf
                }
                None => cur,
            };
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

    Ok(RenderOut {
        meter,
        limiter: limiter.map(|l| {
            let s = l.stats();
            LimiterReport {
                max_gain_reduction_db: s.max_reduction_db(),
                limited_fraction: s.limited_fraction(),
            }
        }),
        clipped,
        denoise: stage.map(|st| {
            let (pause_in, pause_out) = st.pause_levels_db();
            DenoiseOut {
                pause_in,
                pause_out,
                att_pauses: st.stats.attenuation_pauses_db(),
                att_speech: st.stats.attenuation_speech_db(),
            }
        }),
        compressor: comp.map(|c| c.stats()),
        out_format: out_spec.format,
        delta: delta_final,
    })
}

/// Fertig gerenderte Datei: Report und Ausgangsmessung (für den Album-Modus).
struct RenderedFile {
    report: FileReport,
    meter: LoudnessMeter,
}

/// Pass 3 mit festem Gain; mit `correct` folgt bei einer Abweichung vom Ziel
/// über 0,5 LU (Limiter) **eine** Korrekturiteration mit angepasstem Gain.
#[allow(clippy::too_many_arguments)]
fn render(
    prep: &PreparedFile,
    output: &Path,
    settings: &Settings,
    opts: &JobOptions,
    gain_db: f64,
    correct: bool,
    mut warnings: Vec<String>,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<RenderedFile> {
    let started = Instant::now();
    let mut gain_db = gain_db;
    let target = settings.loudness.target_lufs;
    let mut corr = Corrector::new(target, gain_db);
    let correcting = correct && settings.loudness.enabled && settings.limiter.enabled;
    // Mit Limiter-Probe: Der Gain der Probe samt gemessener Lautheit ist der
    // erste Stützpunkt; meist trifft schon der erste Render-Durchlauf.
    if let (true, Some((g0, l0))) = (correcting, prep.probe) {
        gain_db = corr.seed(g0, l0);
    }
    let mut out = render_pass(
        prep,
        output,
        settings,
        opts.export_delta,
        gain_db,
        id,
        progress,
        cancel,
    )?;
    let mut loudness_out = out.meter.stats().map_err(analysis_err)?;
    if correcting {
        while let Some(i) = loudness_out.integrated_lufs {
            let Some(next) = corr.next(gain_db, i) else {
                break;
            };
            progress.pass_added(id, prep.a.frames);
            gain_db = next;
            // Das Delta-WAV hängt nicht vom Gain ab und bleibt aus dem ersten Durchlauf.
            let delta_path = out.delta.take();
            out = render_pass(prep, output, settings, false, gain_db, id, progress, cancel)?;
            out.delta = delta_path;
            loudness_out = out.meter.stats().map_err(analysis_err)?;
        }
    }
    let correction = corr.correction(gain_db);
    progress.file_finished(id);

    if out.clipped > 0 {
        warnings.push(format!(
            "{} Samples beim Quantisieren begrenzt",
            out.clipped
        ));
    }
    if let (true, Some(i)) = (
        correct && settings.loudness.enabled,
        loudness_out.integrated_lufs,
    ) {
        let dev = i - target;
        if dev.abs() > CORRECTION_THRESHOLD_LU {
            warnings.push(format!(
                "Ziel-Lautheit um {dev:+.1} LU verfehlt (Limiter greift stark ein)"
            ));
        }
    }

    let mut denoise_report = prep.denoise_report.clone();
    if let (Some(rep), Some(dn)) = (denoise_report.as_mut(), &out.denoise) {
        rep.pause_level_in_dbfs = dn.pause_in;
        rep.pause_level_out_dbfs = dn.pause_out;
        rep.attenuation_pauses_db = dn.att_pauses;
        rep.attenuation_speech_db = dn.att_speech;
        rep.delta_path = out.delta.clone();
    }

    let mut all = prep.warnings.clone();
    all.append(&mut warnings);
    let mut report = base_report(&prep.input, &prep.a, prep.busy + started.elapsed(), all);
    report.denoise = denoise_report;
    report.leveler = prep
        .curve
        .as_ref()
        .filter(|_| prep.leveler_active)
        .map(|c| c.stats);
    report.highpass_hz = settings
        .highpass
        .enabled
        .then_some(settings.highpass.frequency_hz);
    report.knock = knock_report(&prep.a, settings);
    report.compressor = match (prep.compressor, out.compressor) {
        (Some(p), Some(st)) => Some(CompressorReport {
            threshold_dbfs: p.threshold_dbfs + gain_db,
            ratio: p.ratio,
            max_gain_reduction_db: st.max_reduction_db,
            active_fraction: st.active_fraction(),
        }),
        _ => None,
    };
    report.gate = prep.gate.as_ref().map(|(mask, pause_db)| GateReport {
        floor_dbfs: settings.gate.floor_dbfs,
        pause_level_dbfs: pause_db.map(|p| p.out_db + gain_db),
        attenuation_db: prep.gate_depth(settings, gain_db).unwrap_or(0.0),
        gated_fraction: if mask.is_empty() {
            0.0
        } else {
            mask.iter().filter(|m| **m > 0.5).count() as f64 / mask.len() as f64
        },
    });
    report.normalization_gain_db = Some(gain_db);
    report.loudness_correction_db = correction;
    report.limiter = out.limiter;
    report.output = Some(OutputInfo {
        path: output.to_path_buf(),
        format: out.out_format.label().to_string(),
        clipped_samples: out.clipped,
    });
    report.loudness_out = Some(loudness_out);
    Ok(RenderedFile {
        report,
        meter: out.meter,
    })
}

/// Komplette Verarbeitung einer Datei: analysieren, entrauschen, leveln,
/// normalisieren, limitieren, schreiben (Lautheit pro Datei).
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
    let frames = open(input)?.total_frames();
    progress.file_started(id, frames, prepare_passes(settings) + 1);
    let prep = prepare(input, settings, opts, true, id, progress, cancel)?;
    let mut warnings = Vec::new();
    let gain = normalization_gain(settings, prep.loudness_lufs(), &mut warnings);
    let r = render(
        &prep, output, settings, opts, gain, true, warnings, id, progress, cancel,
    )?;
    Ok(r.report)
}

/// Album-Modus: Ergebnis aller Dateien und gemeinsame Werte.
pub struct AlbumOutcome {
    /// In Eingabereihenfolge.
    pub files: Vec<Result<FileReport>>,
    pub album: AlbumReport,
}

/// Album-Modus: **ein** Gain für alle Dateien, sodass die gemeinsame
/// integrierte Lautheit das Ziel trifft; relative Unterschiede bleiben.
///
/// Ablauf: Pass 1 und 2 für alle Dateien (parallel) → Barriere → gemeinsamer
/// Gain → Pass 3 für alle. Verfehlt die Album-Lautheit der Ausgabe das Ziel um
/// mehr als 0,5 LU (Limiter), werden alle Dateien einmal mit korrigiertem
/// gemeinsamem Gain neu gerendert. Dateien mit Fehler zählen nicht mit.
pub fn process_album(
    items: Vec<(PathBuf, PathBuf)>,
    settings: &Settings,
    opts: &JobOptions,
    jobs: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> AlbumOutcome {
    let n = items.len();
    let prepared: Vec<Result<PreparedFile>> = run_batch(items.iter().collect(), jobs, |id, it| {
        let frames = open(&it.0)?.total_frames();
        progress.file_started(id, frames, prepare_passes(settings));
        let r = prepare(&it.0, settings, opts, false, id, progress, cancel);
        progress.file_finished(id);
        r
    });

    let ok: Vec<&PreparedFile> = prepared.iter().filter_map(|p| p.as_ref().ok()).collect();
    let mut album_warnings = Vec::new();
    let loudness_in = if ok.is_empty() || cancel.is_cancelled() {
        None
    } else {
        LoudnessMeter::integrated_multiple(ok.iter().map(|p| &p.measured))
            .ok()
            .flatten()
    };
    let mut gain_db = normalization_gain(settings, loudness_in, &mut album_warnings);
    if ok.len() < n && !cancel.is_cancelled() {
        album_warnings.push(format!(
            "{} von {n} Dateien fehlgeschlagen; sie zählen nicht zur Album-Lautheit",
            n - ok.len()
        ));
    }

    let render_all = |gain_db: f64, files: &[Option<RenderedFile>]| {
        run_batch(
            (0..n).collect::<Vec<_>>(),
            jobs,
            |id, i| -> Option<Result<RenderedFile>> {
                // Nach der Korrektur nur neu rendern, was beim ersten Mal gelang.
                if !files.is_empty() && files[i].is_none() {
                    return None;
                }
                let prep = prepared[i].as_ref().ok()?;
                if cancel.is_cancelled() {
                    return Some(Err(PipelineError::Cancelled));
                }
                progress.file_started(id, prep.a.frames, 1);
                Some(render(
                    prep,
                    &items[i].1,
                    settings,
                    &JobOptions {
                        export_delta: opts.export_delta && files.is_empty(),
                        ..opts.clone()
                    },
                    gain_db,
                    false,
                    Vec::new(),
                    id,
                    progress,
                    cancel,
                ))
            },
        )
    };

    let measure_out = |r: &[Option<Result<RenderedFile>>]| {
        let meters: Vec<&LoudnessMeter> = r
            .iter()
            .filter_map(|x| x.as_ref()?.as_ref().ok())
            .map(|f| &f.meter)
            .collect();
        if meters.is_empty() {
            None
        } else {
            LoudnessMeter::integrated_multiple(meters.into_iter())
                .ok()
                .flatten()
        }
    };
    let mut rendered = render_all(gain_db, &[]);
    let mut loudness_out = measure_out(&rendered);
    let target = settings.loudness.target_lufs;
    let mut corr = Corrector::new(target, gain_db);
    while settings.loudness.enabled && settings.limiter.enabled && !cancel.is_cancelled() {
        let Some(next) = loudness_out.and_then(|i| corr.next(gain_db, i)) else {
            break;
        };
        gain_db = next;
        let prev: Vec<Option<RenderedFile>> = rendered
            .into_iter()
            .map(|r| r.and_then(|r| r.ok()))
            .collect();
        let again = render_all(gain_db, &prev);
        // Delta-WAV-Pfade und bisherige Render-Zeit übernehmen.
        rendered = again
            .into_iter()
            .zip(prev)
            .enumerate()
            .map(|(i, (s, f))| {
                let busy = prepared[i].as_ref().map_or(0.0, |p| p.busy.as_secs_f64());
                s.map(|r| {
                    r.map(|mut r| {
                        if let Some(f) = f {
                            r.report.elapsed_s += f.report.elapsed_s - busy;
                            if let (Some(a), Some(b)) =
                                (r.report.denoise.as_mut(), f.report.denoise)
                            {
                                a.delta_path = b.delta_path;
                            }
                        }
                        r
                    })
                })
            })
            .collect();
        loudness_out = measure_out(&rendered);
    }
    let correction = corr.correction(gain_db);
    if let (true, Some(i)) = (settings.loudness.enabled, loudness_out) {
        let dev = i - target;
        if dev.abs() > CORRECTION_THRESHOLD_LU {
            album_warnings.push(format!(
                "Album-Lautheit um {dev:+.1} LU verfehlt (Limiter greift stark ein)"
            ));
        }
    }

    let files: Vec<Result<FileReport>> = prepared
        .into_iter()
        .zip(rendered)
        .map(|(p, r)| match (p, r) {
            (Err(e), _) => Err(e),
            (Ok(_), Some(r)) => r.map(|r| r.report),
            (Ok(_), None) => Err(PipelineError::Cancelled),
        })
        .collect();
    let rendered_files = files.iter().filter(|f| f.is_ok()).count();
    AlbumOutcome {
        files,
        album: AlbumReport {
            files: rendered_files,
            loudness_before_gain_lufs: loudness_in,
            gain_db: settings.loudness.enabled.then_some(gain_db),
            loudness_correction_db: correction,
            loudness_out_lufs: loudness_out,
            warnings: album_warnings,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Corrector, span};

    /// Modell eines Limiters: oberhalb von −18 LUFS wirkt jedes dB Gain nur halb.
    fn limited(gain: f64) -> f64 {
        let l = -26.0 + gain;
        if l > -18.0 {
            -18.0 + (l + 18.0) * 0.5
        } else {
            l
        }
    }

    #[test]
    fn corrector_converges_with_compression() {
        let mut gain = 10.0; // ideal ohne Limiter: −16
        let mut c = Corrector::new(-16.0, gain);
        let mut renders = 1;
        while let Some(g) = c.next(gain, limited(gain)) {
            gain = g;
            renders += 1;
        }
        let out = limited(gain);
        assert!(
            (out + 16.0).abs() <= 0.5,
            "{out:.2} nach {renders} Durchläufen"
        );
        assert!(renders <= 3);
        assert!(c.correction(gain).unwrap() > 0.0);

        // Treffer im ersten Durchlauf: keine Iteration.
        let mut c = Corrector::new(-16.0, 5.0);
        assert_eq!(c.next(5.0, -16.2), None);
        assert_eq!(c.correction(5.0), None);
    }

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
