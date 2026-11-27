//! Leveled Audio – einfache Oberfläche: importieren, einstellen, exportieren.
// Unter Windows kein Konsolenfenster öffnen.
#![cfg_attr(windows, windows_subsystem = "windows")]

use leveled_audio_gui::app::{APP_NAME, App};

fn main() -> eframe::Result {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        // Klein starten: Eingabe, Preset und Export passen hinein, die
        // Einstellungen klappen auf und scrollen.
        .with_inner_size([600.0, 460.0])
        .with_min_inner_size([460.0, 360.0])
        .with_drag_and_drop(true);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png"))
    {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        APP_NAME,
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
