//! leveled-audio-gui – einfache Oberfläche zum Testen: importieren, einstellen, exportieren.
// Unter Windows kein Konsolenfenster öffnen.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("leveled-audio")
            .with_inner_size([960.0, 820.0])
            .with_min_inner_size([720.0, 520.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "leveled-audio",
        options,
        Box::new(|cc| Ok(Box::new(leveled_audio_gui::app::App::new(cc)))),
    )
}
