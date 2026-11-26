//! Verarbeitung einer einzelnen Datei.

use crate::progress::{CancelToken, Pass, ProgressSink};
use crate::report::{FileReport, InputInfo, LimiterReport, OutputInfo};
use crate::settings::{BitDepth, DitherMode, Settings};
use crate::{BLOCK_FRAMES, PipelineError, Result};
use leveled_audio_analysis::{LoudnessMeter, LoudnessStats};
use leveled_audio_dsp::{LimiterParams, TpdfDither, TruePeakLimiter, apply_gain, db_to_lin};
use leveled_audio_io::{AudioSource, AudioSpec, SampleFormat};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Ab dieser Aussteuerung gilt ein Sample als „an der Grenze“ (1 LSB bei 16 bit).
const CLIP_LEVEL: f32 = 1.0 - 1.0 / 32768.0;
const CLIP_RUN: u32 = 3;

struct InputAnalysis {
    spec: AudioSpec,
    frames: u64,
    loudness: LoudnessStats,
    clip_runs: u64,
    dc_dbfs: Vec<Option<f64>>,
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

/// Pass 1: Eingabe messen.
fn analysis_pass(
    src: &mut dyn AudioSource,
    path: &Path,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<InputAnalysis> {
    let spec = src.spec();
    let ch = spec.channels as usize;
    let mut meter = LoudnessMeter::new(ch, spec.sample_rate).map_err(analysis_err)?;
    let mut buf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut run = vec![0u32; ch];
    let mut clip_runs = 0u64;
    let mut sums = vec![0f64; ch];
    let mut frames = 0u64;
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
        frames += n as u64;
        progress.advance(id, Pass::Analyze, n as u64);
    }
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

fn finish_report(
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

/// Nur messen, nichts schreiben.
pub fn analyze_file(
    path: &Path,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<FileReport> {
    let started = Instant::now();
    let mut src = open(path)?;
    progress.file_started(id, src.total_frames(), 1);
    let a = analysis_pass(src.as_mut(), path, id, progress, cancel)?;
    progress.file_finished(id);
    Ok(finish_report(path, &a, started, Vec::new()))
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

fn partial_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    output.with_file_name(name)
}

/// Entfernt die Teildatei, falls die Verarbeitung nicht erfolgreich endet.
struct PartialGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Komplette Verarbeitung: messen, normalisieren, limitieren, schreiben.
///
/// Geschrieben wird zuerst `<ausgabe>.partial`, erst nach Erfolg wird
/// umbenannt. Bei Fehler oder Abbruch wird die Teildatei gelöscht.
pub fn process_file(
    input: &Path,
    output: &Path,
    settings: &Settings,
    id: usize,
    progress: &dyn ProgressSink,
    cancel: &CancelToken,
) -> Result<FileReport> {
    let started = Instant::now();
    let mut src = open(input)?;
    let total = src.total_frames();
    progress.file_started(id, total, 2);

    // ---- Pass 1: Analyse
    let a = analysis_pass(src.as_mut(), input, id, progress, cancel)?;
    let mut warnings = Vec::new();

    let gain_db = if settings.loudness.enabled {
        match a.loudness.integrated_lufs {
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
            "sehr hohe Anhebung um {gain_db:.1} dB; Rauschen wird entsprechend lauter"
        ));
    }

    // ---- Pass 2: Rendern
    let out_spec = resolve_output_spec(a.spec, settings.output.bit_depth);
    let ch = a.spec.channels as usize;
    let dither_on = match settings.output.dither {
        DitherMode::Auto => out_spec.format == SampleFormat::Int16,
        DitherMode::On => !out_spec.format.is_float(),
        DitherMode::Off => false,
    };
    let mut dither = dither_on.then(|| TpdfDither::new(ch, out_spec.format.bits()));
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
    let latency = limiter.as_ref().map_or(0, |l| l.latency()) as u64;

    let partial = partial_path(output);
    let mut guard = PartialGuard {
        path: partial.clone(),
        armed: true,
    };
    let mut sink = leveled_audio_io::create_sink(&partial, out_spec).map_err(audio_err(output))?;
    let mut meter = LoudnessMeter::new(ch, a.spec.sample_rate).map_err(analysis_err)?;

    let gain = db_to_lin(gain_db) as f32;
    let mut inbuf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut outbuf = vec![Vec::with_capacity(BLOCK_FRAMES); ch];
    let mut fed = 0u64; // Frames in den Limiter gegeben (inkl. Nachlauf-Nullen)
    let mut written = 0u64;
    src.seek(0).map_err(audio_err(input))?;
    while written < a.frames {
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let mut n = src
            .read(&mut inbuf, BLOCK_FRAMES)
            .map_err(audio_err(input))?;
        if n == 0 {
            // Nachlauf: Nullen einspeisen, bis die Latenz aufgeholt ist.
            n = (a.frames + latency - fed).min(BLOCK_FRAMES as u64) as usize;
            for c in inbuf.iter_mut() {
                c.clear();
                c.resize(n, 0.0);
            }
        } else {
            progress.advance(id, Pass::Render, n as u64);
        }
        apply_gain(&mut inbuf, n, gain);
        let block = match limiter.as_mut() {
            Some(l) => {
                l.process(&inbuf, &mut outbuf, n);
                &mut outbuf
            }
            None => &mut inbuf,
        };
        // Vorlauf (Latenz) verwerfen.
        let skip = latency.saturating_sub(fed).min(n as u64) as usize;
        fed += n as u64;
        let take = ((n - skip) as u64).min(a.frames - written) as usize;
        if take == 0 {
            continue;
        }
        if skip > 0 {
            for c in block.iter_mut() {
                c.drain(..skip);
            }
        }
        if let Some(d) = dither.as_mut() {
            d.apply(block, take);
        }
        sink.write(block, take).map_err(audio_err(output))?;
        meter.add(block, take).map_err(analysis_err)?;
        written += take as u64;
    }
    let clipped = sink.clipped_samples();
    sink.finalize().map_err(audio_err(output))?;
    std::fs::rename(&partial, output).map_err(|e| PipelineError::Io {
        path: output.to_path_buf(),
        message: e.to_string(),
    })?;
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

    let mut report = finish_report(input, &a, started, warnings);
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
