//! Tempo der Rauschreduktion (STFT + Denoiser + Rücktransformation), ein Kern.
//! Aufruf: cargo run --release -p leveled-audio-dsp --example denoise_speed
use leveled_audio_dsp::{DenoiseParams, SpectralDenoiser, StftEngine};
use std::time::Instant;

fn main() {
    let rate = 48_000usize;
    let secs = 120;
    let mut s = 1u64;
    let x: Vec<f32> = (0..rate * secs)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        })
        .collect();
    let input = [x.clone(), x];
    let noise = vec![vec![0.01f32; 1025]; 2];
    let mut den = SpectralDenoiser::new(&noise, 512.0 / 48_000.0, DenoiseParams::default());
    let mut eng = StftEngine::new(2, 2048, true);
    let mut out = vec![Vec::new(), Vec::new()];
    let block = 4096;
    let t = Instant::now();
    let mut pos = 0;
    while pos < rate * secs {
        let n = block.min(rate * secs - pos);
        let b: Vec<Vec<f32>> = input.iter().map(|c| c[pos..pos + n].to_vec()).collect();
        eng.process(&b, n, &mut out, |f| den.process(f.spectra));
        pos += n;
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "Stereo 48 kHz, {secs} s Audio in {el:.2} s ⇒ {:.0}× Echtzeit (ein Kern)",
        secs as f64 / el
    );
}
