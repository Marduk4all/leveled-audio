//! Diagnose: Frame-Features, Hintergrund und VAD-Klassen einer Datei ausgeben.
//! Aufruf: cargo run --release -p leveled-audio-pipeline --example vad_dump -- <datei.wav>
use leveled_audio_analysis::{
    FeatureExtractor, FrameClass, VadParams, classify, vad::sliding_percentile,
};
use leveled_audio_dsp::{StftEngine, auto_fft_size};

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).expect("Datei"));
    let mut src = leveled_audio_io::open_source(&path).unwrap();
    let spec = src.spec();
    let ch = spec.channels as usize;
    let n = auto_fft_size(spec.sample_rate);
    let mut eng = StftEngine::new(ch, n, false);
    let mut fx = FeatureExtractor::new(spec.sample_rate, n, ch);
    let mut buf = vec![Vec::new(); ch];
    loop {
        let k = src.read(&mut buf, 4096).unwrap();
        if k == 0 {
            break;
        }
        eng.process(&buf, k, &mut [], |f| fx.push(f.spectra, f.time));
    }
    let f = fx.finish();
    let p = VadParams::default();
    let silence: Vec<bool> = (0..f.len())
        .map(|k| f.flat_frac[k] > 0.5 || f.e_db[k] < p.silence_db)
        .collect();
    let bg = sliding_percentile(
        &f.e_db,
        &silence,
        f.frames_for(p.background_half_window_s),
        p.background_percentile,
    );
    let cls = classify(&f, &p);
    let hop_ms = f.hop_seconds() * 1000.0;
    for k in (0..f.len()).step_by(3) {
        let c = match cls[k] {
            FrameClass::Speech => 'S',
            FrameClass::Pause => '.',
            FrameClass::Other => 'o',
            FrameClass::Silence => '_',
        };
        let bar = ((f.e_db[k] - bg[k]).max(0.0) / 1.5) as usize;
        println!(
            "{:7.0}ms {c} e{:6.1} bg{:6.1} fl{:4.2} fx{:4.1} {}",
            k as f64 * hop_ms,
            f.e_db[k],
            bg[k],
            f.flatness[k],
            f.flux[k],
            "#".repeat(bar.min(40))
        );
    }
}
