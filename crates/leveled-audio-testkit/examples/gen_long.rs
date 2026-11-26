//! Erzeugt eine lange Testdatei blockweise (konstanter Speicher).
//! Aufruf: cargo run --release -p leveled-audio-testkit --example gen_long -- <datei.wav> <minuten>
use leveled_audio_io::{AudioSpec, SampleFormat, create_sink};
use leveled_audio_testkit::{mix, speechlike, white_noise};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("Pfad"));
    let minutes: usize = args.next().expect("Minuten").parse().expect("Zahl");
    let rate = 48_000;
    let spec = AudioSpec {
        sample_rate: rate,
        channels: 2,
        format: SampleFormat::Int24,
    };
    let mut sink = create_sink(&path, spec).expect("anlegen");
    for m in 0..minutes {
        // Jede Minute leicht anderer Pegel (Sprecherwechsel), Rauschen −60 dBFS.
        let level = -28.0 + 6.0 * ((m as f64) * 0.7).sin();
        let s = speechlike(rate, 2, level, 60.0, m as u64 + 1);
        let n = white_noise(rate, 2, -60.0, 60.0, m as u64 + 1000);
        let block = mix(&s, &n, 0.0);
        sink.write(&block, block[0].len()).expect("schreiben");
    }
    sink.finalize().expect("abschließen");
}
