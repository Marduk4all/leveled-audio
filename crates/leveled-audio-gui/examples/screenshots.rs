//! Screenshots der Oberfläche für README und Anleitung (Englisch und Deutsch).
//!
//! Rendert die echte GUI ohne Fenster (egui_kittest + wgpu) und schreibt PNGs.
//!
//! Aufruf:
//!   cargo run --release -p leveled-audio-gui --example screenshots -- <Arbeitsordner> [Zielordner]
//! Der Arbeitsordner nimmt die erzeugten Demo-WAVs und deren Exporte auf;
//! Zielordner ist standardmäßig `docs/images`.

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use leveled_audio_gui::app::App;
use leveled_audio_gui::i18n::Lang;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SIZE: [f32; 2] = [600.0, 640.0];
const SCALE: f32 = 2.0;

fn demo_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("Arbeitsordner");
    let mut v = Vec::new();
    for (name, level, seed) in [("interview.wav", -26.0, 3), ("episode-01.wav", -33.0, 7)] {
        let p = dir.join(name);
        leveled_audio_testkit::write_wav(
            &p,
            48_000,
            leveled_audio_io::SampleFormat::Int24,
            &leveled_audio_testkit::speechlike(48_000, 2, level, 20.0, seed),
        );
        // Ältere Exporte entfernen, damit keine Rückfrage erscheint.
        let _ = std::fs::remove_file(leveled_audio_gui::job::output_path(&p, "podcast-stereo"));
        v.push(p);
    }
    v
}

fn save(h: &mut Harness<'_, App>, path: &Path) {
    // Mauszeiger aus dem Bild nehmen.
    h.event(egui::Event::PointerGone);
    steps(h, 4);
    let img = h.render().expect("Rendern");
    img.save(path).expect("PNG schreiben");
    println!("{}", path.display());
}

fn steps(h: &mut Harness<'_, App>, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let work = PathBuf::from(args.next().expect("Arbeitsordner angeben"));
    let out = PathBuf::from(args.next().unwrap_or_else(|| "docs/images".into()));
    std::fs::create_dir_all(&out).expect("Zielordner");

    for (lang, tag) in [(Lang::En, "en"), (Lang::De, "de")] {
        let files = demo_files(&work.join(tag));
        let mut h = Harness::builder()
            .with_size(SIZE)
            .with_pixels_per_point(SCALE)
            .wgpu()
            .with_accessibility_check(false)
            .build_eframe(|cc| {
                // Wie eframe in der echten App: fehlende Zeichen (▶, Emoji)
                // aus den Systemschriften holen.
                cc.egui_ctx.add_font_provider(std::sync::Arc::new(
                    egui_system_font_provider::SystemFontProvider::new(),
                ));
                let mut app = App::new(cc);
                app.lang = lang;
                app.add_inputs(files.clone());
                app
            });
        steps(&mut h, 3);
        // Eine Einstellungsgruppe aufklappen.
        h.get_by_label("Leveler").click();
        steps(&mut h, 8);
        save(&mut h, &out.join(format!("gui-settings-{tag}.png")));

        // Leveler wieder zu, exportieren und das Ergebnis zeigen.
        h.get_by_label("Leveler").click();
        steps(&mut h, 8);
        let export = if lang == Lang::En {
            "▶ Export"
        } else {
            "▶ Exportieren"
        };
        h.get_by_label(export).click();
        steps(&mut h, 2);
        assert!(h.state().is_running(), "Export nicht gestartet");
        let t0 = Instant::now();
        while !h.state().is_finished() && t0.elapsed() < Duration::from_secs(120) {
            std::thread::sleep(Duration::from_millis(50));
            h.step();
        }
        assert!(h.state().is_finished(), "Export nicht fertig");
        h.set_size(egui::vec2(SIZE[0], 700.0));
        steps(&mut h, 8);
        save(&mut h, &out.join(format!("gui-result-{tag}.png")));
    }
}
