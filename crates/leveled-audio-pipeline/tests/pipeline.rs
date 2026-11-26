//! Integrationstests der Pipeline (Phase-1-Akzeptanzkriterien).

use leveled_audio_io::SampleFormat;
use leveled_audio_pipeline::{
    BitDepth, CancelToken, DitherMode, JobOptions, NoProgress, PipelineError, Settings,
    process_file,
};
use leveled_audio_testkit::{
    Planar, amp_to_db, mix, read_wav, reference_true_peak, speechlike, temp_dir, white_noise,
    write_wav,
};
use std::path::Path;

fn process(input: &Path, output: &Path, settings: &Settings) -> leveled_audio_pipeline::FileReport {
    process_file(
        input,
        output,
        settings,
        &JobOptions::default(),
        0,
        &NoProgress,
        &CancelToken::new(),
    )
    .unwrap()
}

fn speech_file(
    dir: &Path,
    name: &str,
    rate: u32,
    ch: usize,
    level: f64,
    format: SampleFormat,
) -> std::path::PathBuf {
    let s = speechlike(rate, ch, level, 30.0, 42);
    let n = white_noise(rate, ch, level - 35.0, 30.0, 7);
    let path = dir.join(name);
    write_wav(&path, rate, format, &mix(&s, &n, 0.0));
    path
}

#[test]
fn normalizes_to_target_and_respects_true_peak() {
    let dir = temp_dir("normalize");
    for (target, preset) in [(-16.0, "podcast-stereo"), (-23.0, "ebu-r128")] {
        let input = speech_file(&dir, "in.wav", 48_000, 2, -30.0, SampleFormat::Int24);
        let output = dir.join(format!("out_{preset}.wav"));
        let settings = Settings::preset(preset).unwrap();
        let r = process(&input, &output, &settings);

        let out = r.loudness_out.unwrap();
        let i = out.integrated_lufs.unwrap();
        assert!((i - target).abs() <= 0.5, "{preset}: {i:.2} LUFS");
        assert!(
            out.true_peak_dbtp.unwrap() <= -1.0,
            "ebur128 TP {:?}",
            out.true_peak_dbtp
        );

        // Unabhängige 16×-Referenz auf den ersten 10 s.
        let (_, data) = read_wav(&output);
        let head: Planar = data.iter().map(|c| c[..480_000].to_vec()).collect();
        let tp = amp_to_db(reference_true_peak(&head));
        assert!(tp <= -0.9, "Referenz-TP {tp:.3}");
        assert_eq!(data[0].len(), 30 * 48_000);
    }
}

#[test]
fn deterministic_byte_identical_output() {
    let dir = temp_dir("determinism");
    let input = speech_file(&dir, "in.wav", 44_100, 2, -25.0, SampleFormat::Int16);
    let s = Settings::default();
    process(&input, &dir.join("a.wav"), &s);
    process(&input, &dir.join("b.wav"), &s);
    let a = std::fs::read(dir.join("a.wav")).unwrap();
    let b = std::fs::read(dir.join("b.wav")).unwrap();
    assert_eq!(a, b);
}

#[test]
fn all_formats_keep_length_and_format() {
    let dir = temp_dir("formats");
    for format in [
        SampleFormat::Int16,
        SampleFormat::Int24,
        SampleFormat::Int32,
        SampleFormat::Float32,
    ] {
        for ch in [1usize, 2] {
            for rate in [44_100u32, 48_000, 96_000] {
                let input = dir.join("in.wav");
                let s = speechlike(rate, ch, -28.0, 3.0, 1);
                write_wav(&input, rate, format, &s);
                let output = dir.join("out.wav");
                let r = process(&input, &output, &Settings::default());
                let (spec, data) = read_wav(&output);
                assert_eq!(data[0].len(), s[0].len(), "{format:?} {ch} {rate}");
                assert_eq!(spec.sample_rate, rate);
                assert_eq!(spec.channels as usize, ch);
                let expected = if format == SampleFormat::Int32 {
                    SampleFormat::Int24
                } else {
                    format
                };
                assert_eq!(spec.format, expected);
                assert_eq!(r.output.unwrap().clipped_samples, 0);
            }
        }
    }
}

#[test]
fn passthrough_is_bit_exact() {
    let dir = temp_dir("passthrough");
    let mut s = Settings::default();
    s.denoise.enabled = false;
    s.leveler.enabled = false;
    s.highpass.enabled = false;
    s.compressor.enabled = false;
    s.loudness.enabled = false;
    s.limiter.enabled = false;
    s.output.dither = DitherMode::Off;
    for format in [
        SampleFormat::Int16,
        SampleFormat::Int24,
        SampleFormat::Float32,
    ] {
        let input = speech_file(&dir, "in.wav", 48_000, 2, -20.0, format);
        let output = dir.join("out.wav");
        process(&input, &output, &s);
        assert_eq!(read_wav(&input).1, read_wav(&output).1, "{format:?}");
    }
}

#[test]
fn limiter_alone_does_not_touch_quiet_audio() {
    // Limiter an, aber Signal weit unter dem Ceiling: nach Latenzausgleich identisch.
    let dir = temp_dir("limiter-quiet");
    let mut s = Settings::default();
    s.denoise.enabled = false;
    s.leveler.enabled = false;
    s.highpass.enabled = false;
    s.compressor.enabled = false;
    s.loudness.enabled = false;
    s.output.dither = DitherMode::Off;
    s.output.bit_depth = BitDepth::F32;
    let input = speech_file(&dir, "in.wav", 48_000, 2, -30.0, SampleFormat::Float32);
    let output = dir.join("out.wav");
    let r = process(&input, &output, &s);
    assert_eq!(read_wav(&input).1, read_wav(&output).1);
    assert_eq!(r.limiter.unwrap().limited_fraction, 0.0);
}

#[test]
fn cancel_leaves_no_files() {
    let dir = temp_dir("cancel");
    let input = speech_file(&dir, "in.wav", 48_000, 2, -20.0, SampleFormat::Int16);
    let output = dir.join("out.wav");
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = process_file(
        &input,
        &output,
        &Settings::default(),
        &JobOptions::default(),
        0,
        &NoProgress,
        &cancel,
    )
    .unwrap_err();
    assert!(matches!(err, PipelineError::Cancelled));
    assert!(!output.exists());
    assert!(!dir.join("out.wav.partial").exists());
}

#[test]
fn silent_input_is_reported_not_amplified() {
    let dir = temp_dir("silence");
    let input = dir.join("in.wav");
    write_wav(
        &input,
        48_000,
        SampleFormat::Int16,
        &vec![vec![0.0; 48_000 * 2]; 2],
    );
    let r = process(&input, &dir.join("out.wav"), &Settings::default());
    assert_eq!(r.normalization_gain_db, Some(0.0));
    assert!(r.warnings.iter().any(|w| w.contains("stumm")));
}

#[test]
fn clipped_input_is_detected() {
    let dir = temp_dir("clip");
    let s = speechlike(48_000, 1, -3.0, 5.0, 3);
    let clipped: Planar = vec![s[0].iter().map(|x| x.clamp(-1.0, 1.0)).collect()];
    let input = dir.join("in.wav");
    write_wav(&input, 48_000, SampleFormat::Int16, &clipped);
    let r = process(&input, &dir.join("out.wav"), &Settings::default());
    assert!(r.input_clip_runs > 0);
    assert!(r.warnings.iter().any(|w| w.contains("übersteuert")));
}
