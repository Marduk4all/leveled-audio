//! Barrierefreiheit: Jedes Bedienelement hat einen Namen für Screenreader
//! (AccessKit). egui_kittest prüft das bei jedem Schritt und bricht sonst ab.
//! Geprüft wird in beiden Sprachen, mit allen Einstellungsgruppen aufgeklappt.

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use leveled_audio_gui::app::App;
use leveled_audio_gui::i18n::Lang;
use leveled_audio_pipeline::{PARAM_GROUPS, PARAMS};

fn harness(lang: Lang) -> Harness<'static, App> {
    Harness::builder()
        .with_size(egui::vec2(600.0, 2400.0))
        // Geprüft wird die Beschriftung, nicht die Schriften des CI-Rechners.
        .allow_missing_glyphs()
        .build_eframe(move |cc| {
            // Wie in der echten App: fehlende Zeichen aus Systemschriften.
            cc.egui_ctx.add_font_provider(std::sync::Arc::new(
                egui_system_font_provider::SystemFontProvider::new(),
            ));
            let mut app = App::new(cc);
            app.lang = lang;
            app
        })
}

#[test]
fn every_control_has_an_accessible_name() {
    for lang in [Lang::En, Lang::De] {
        let mut h = harness(lang);
        h.run_steps(2);
        for group in PARAM_GROUPS {
            h.get_by_label(lang.group(group)).click();
            h.run_steps(2);
        }
        // Alle „Feineinstellungen“ aufklappen (von oben nach unten, eine je Schritt).
        let fine = lang.t("Feineinstellungen", "Fine tuning");
        loop {
            let closed = h
                .query_all_by_label(fine)
                .find(|n| n.accesskit_node().data().is_expanded() == Some(false));
            let Some(node) = closed else { break };
            node.click();
            h.run_steps(2);
        }
        // Der Accessibility-Check läuft in jedem Schritt; hier noch einmal explizit.
        h.run_steps(2);
        // Alle Parameter müssen jetzt sichtbar (also mitgeprüft) sein.
        for p in PARAMS {
            let label = lang.param(p).0;
            assert!(
                h.query_all_by_label(label).next().is_some(),
                "{lang:?}: Parameter {} ({label}) nicht sichtbar",
                p.key
            );
        }
    }
}
