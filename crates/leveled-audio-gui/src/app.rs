//! Oberfläche (egui): ein Fenster mit Eingabe, Einstellungen, Verarbeitung
//! und Ergebnis (PLAN Phase 6). Exportiert wird neben die Originaldateien
//! (`<Name>-leveled-<Preset>.wav`); vorhandene Ziele nur nach Rückfrage.

use crate::i18n::Lang;
use crate::job::{self, Existing, Outcome, Request};
use eframe::egui;
use leveled_audio_cli::summary::{FileOutcome, JsonMeta, Status, annotate_preset, write_json};
use leveled_audio_pipeline::{
    CancelToken, PARAM_GROUPS, PARAMS, PRESET_NAMES, ParamKind, ParamSpec, Pass, ProgressSink,
    Settings, lookup, settings_table,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Name des Programms (Fenstertitel, App-Bundle).
pub const APP_NAME: &str = "Leveled Audio";

/// Farben der Oberfläche (passend zum Programm-Icon).
mod theme {
    use eframe::egui::{Color32, CornerRadius, Stroke, Visuals};

    /// Indigo (Icon oben).
    pub const ACCENT: Color32 = Color32::from_rgb(0x4C, 0x3B, 0xCF);
    /// Türkis (Icon unten).
    pub const TEAL: Color32 = Color32::from_rgb(0x0E, 0x8F, 0x84);
    pub const GOLD: Color32 = Color32::from_rgb(0xE8, 0xA3, 0x1A);
    pub const OK: Color32 = Color32::from_rgb(0x1B, 0x8A, 0x4B);
    pub const ERROR: Color32 = Color32::from_rgb(0xC6, 0x28, 0x28);
    /// Hintergrund der Export-Leiste.
    pub const BAR: Color32 = Color32::from_rgb(0xEC, 0xEA, 0xFB);

    pub fn visuals() -> Visuals {
        let mut v = Visuals::light();
        v.panel_fill = Color32::from_rgb(0xF7, 0xF7, 0xFC);
        v.window_fill = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(0xF0, 0xEF, 0xFA);
        v.extreme_bg_color = Color32::WHITE;
        v.hyperlink_color = ACCENT;
        v.error_fg_color = ERROR;
        v.warn_fg_color = GOLD;
        v.selection.bg_fill = Color32::from_rgb(0xC9, 0xC3, 0xF5);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        let r = CornerRadius::same(6);
        let w = &mut v.widgets;
        w.inactive.weak_bg_fill = Color32::from_rgb(0xE6, 0xE4, 0xF7);
        w.inactive.bg_fill = Color32::from_rgb(0xD9, 0xD5, 0xF5);
        w.hovered.weak_bg_fill = Color32::from_rgb(0xD6, 0xD1, 0xF6);
        w.hovered.bg_fill = Color32::from_rgb(0xC9, 0xC3, 0xF5);
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.weak_bg_fill = Color32::from_rgb(0xB8, 0xB0, 0xF0);
        w.active.bg_fill = ACCENT;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = r;
        }
        v.slider_trailing_fill = true;
        v
    }
}

/// Abschnittsüberschrift mit farbiger Nummer.
fn section(ui: &mut egui::Ui, nr: &str, title: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(theme::ACCENT)
            .corner_radius(egui::CornerRadius::same(11))
            .inner_margin(egui::Margin::symmetric(7, 1))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(nr).strong().color(egui::Color32::WHITE));
            });
        ui.label(
            egui::RichText::new(title)
                .heading()
                .strong()
                .color(theme::ACCENT),
        );
    });
    ui.add_space(2.0);
}

/// Fortschritt je Datei: (erledigt, gesamt, Durchgang).
#[derive(Default)]
struct ProgressState {
    files: BTreeMap<usize, (u64, u64, &'static str)>,
    done: usize,
}

struct GuiProgress {
    ctx: egui::Context,
    state: Mutex<ProgressState>,
}

impl GuiProgress {
    fn repaint(&self) {
        self.ctx.request_repaint();
    }
}

impl ProgressSink for GuiProgress {
    fn file_started(&self, id: usize, frames: u64, passes: u32) {
        let mut s = self.state.lock().expect("Mutex");
        s.files
            .insert(id, (0, frames * passes as u64, Pass::Analyze.label()));
        drop(s);
        self.repaint();
    }
    fn advance(&self, id: usize, pass: Pass, frames: u64) {
        if let Some(f) = self.state.lock().expect("Mutex").files.get_mut(&id) {
            f.0 += frames;
            f.2 = pass.label();
        }
        self.repaint();
    }
    fn pass_added(&self, id: usize, frames: u64) {
        if let Some(f) = self.state.lock().expect("Mutex").files.get_mut(&id) {
            f.1 += frames;
        }
    }
    fn file_finished(&self, id: usize) {
        self.state.lock().expect("Mutex").files.remove(&id);
        self.repaint();
    }
}

struct Running {
    cancel: CancelToken,
    progress: Arc<GuiProgress>,
    names: Vec<String>,
    result: Arc<Mutex<Option<Outcome>>>,
    started: Instant,
    /// Ordner der ersten Ausgabe (für „Ordner öffnen“).
    folder: Option<PathBuf>,
}

struct Finished {
    outcome: Outcome,
    settings: Settings,
    folder: Option<PathBuf>,
    cancelled: bool,
    seconds: f64,
}

pub struct App {
    /// Sprache der Oberfläche (Start: Englisch).
    pub lang: Lang,
    inputs: Vec<PathBuf>,
    recursive: bool,
    wav_count: Result<usize, String>,
    settings: Settings,
    /// Grundlage für „Zurücksetzen“ (eingebautes Preset oder geladene Datei).
    base: Settings,
    preset_label: String,
    jobs: usize,
    running: Option<Running>,
    finished: Option<Finished>,
    message: Option<(bool, String)>,
    /// Offene Rückfrage: diese Zieldateien existieren schon.
    confirm: Option<Vec<PathBuf>>,
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(4)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(theme::visuals());
        let settings = Settings::default();
        App {
            lang: Lang::default(),
            inputs: Vec::new(),
            recursive: true,
            wav_count: Ok(0),
            base: settings.clone(),
            settings,
            preset_label: "podcast-stereo".into(),
            jobs: default_jobs(),
            running: None,
            finished: None,
            message: None,
            confirm: None,
        }
    }

    fn add_inputs(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            if !self.inputs.contains(&p) {
                self.inputs.push(p);
            }
        }
        self.recount();
    }

    fn recount(&mut self) {
        self.wav_count =
            job::count_inputs(&self.inputs, self.recursive).map_err(|e| format!("{e:#}"));
    }

    fn request(&self) -> Request {
        Request {
            inputs: self.inputs.clone(),
            recursive: self.recursive,
            settings: self.settings.clone(),
            preset_label: self.preset_label.clone(),
            jobs: self.jobs,
        }
    }

    /// „Start“: erst prüfen, ob Zieldateien schon existieren (dann Rückfrage).
    fn start(&mut self, ctx: &egui::Context) {
        self.message = None;
        match job::existing_outputs(&self.request()) {
            Ok(v) if !v.is_empty() => self.confirm = Some(v),
            Ok(_) => self.launch(ctx, Existing::Error),
            Err(e) => self.message = Some((true, format!("{e:#}"))),
        }
    }

    fn launch(&mut self, ctx: &egui::Context, existing: Existing) {
        self.confirm = None;
        let req = self.request();
        let plan = match job::plan(&req, existing) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some((true, format!("{e:#}")));
                return;
            }
        };
        let cancel = CancelToken::new();
        let progress = Arc::new(GuiProgress {
            ctx: ctx.clone(),
            state: Mutex::new(ProgressState::default()),
        });
        let result = Arc::new(Mutex::new(None));
        let names = plan.names.clone();
        let folder = plan.jobs.first().and_then(|j| match j {
            leveled_audio_cli::inputs::Job::Process { output, .. }
            | leveled_audio_cli::inputs::Job::Skip { output, .. } => {
                output.parent().map(Path::to_path_buf)
            }
        });
        {
            let (cancel, progress, result, ctx) = (
                cancel.clone(),
                progress.clone(),
                result.clone(),
                ctx.clone(),
            );
            std::thread::spawn(move || {
                let p = progress.clone();
                let on_done = move |_id: usize| {
                    p.state.lock().expect("Mutex").done += 1;
                    p.repaint();
                };
                let out = job::execute(&req, plan, &*progress, &cancel, &on_done);
                *result.lock().expect("Mutex") = Some(out);
                ctx.request_repaint();
            });
        }
        self.finished = None;
        self.running = Some(Running {
            cancel,
            progress,
            names,
            result,
            started: Instant::now(),
            folder,
        });
    }

    fn poll(&mut self) {
        let done = self
            .running
            .as_ref()
            .and_then(|r| r.result.lock().expect("Mutex").take());
        // `take()` erst, wenn das Ergebnis da ist (sonst ginge der laufende
        // Auftrag samt Status und Abbrechen-Knopf schon im nächsten Frame verloren).
        if let Some(outcome) = done
            && let Some(r) = self.running.take()
        {
            self.finished = Some(Finished {
                outcome,
                settings: self.settings.clone(),
                folder: r.folder,
                cancelled: r.cancel.is_cancelled(),
                seconds: r.started.elapsed().as_secs_f64(),
            });
        }
    }

    // ---------------------------------------------------------------- UI

    fn ui_input(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(l.t("Dateien hinzufügen…", "Add files…"))
                .clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .add_filter("WAV", &["wav", "WAV"])
                    .pick_files()
            {
                self.add_inputs(files);
            }
            if ui
                .button(l.t("Ordner hinzufügen…", "Add folder…"))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.add_inputs([dir]);
            }
            if ui
                .checkbox(
                    &mut self.recursive,
                    l.t("Unterordner einbeziehen", "Include subfolders"),
                )
                .changed()
            {
                self.recount();
            }
            if !self.inputs.is_empty() && ui.button(l.t("Alle entfernen", "Remove all")).clicked() {
                self.inputs.clear();
                self.recount();
            }
        });
        ui.label(
            egui::RichText::new(l.t(
                "Dateien oder Ordner lassen sich auch ins Fenster ziehen.",
                "You can also drag files or folders into the window.",
            ))
            .weak(),
        );
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("inputs")
            .max_height(120.0)
            .show(ui, |ui| {
                for (i, p) in self.inputs.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("✕")
                            .on_hover_text(l.t("Entfernen", "Remove"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        let kind = if p.is_dir() { "📁" } else { "🎵" };
                        ui.label(format!("{kind} {}", p.display()));
                    });
                }
            });
        if let Some(i) = remove {
            self.inputs.remove(i);
            self.recount();
        }
        match &self.wav_count {
            Ok(0) => ui.label(egui::RichText::new(l.t("0 WAV-Dateien", "0 WAV files")).weak()),
            Ok(n) => ui.label(
                egui::RichText::new(format!("{n} {}", l.t("WAV-Datei(en)", "WAV file(s)")))
                    .strong()
                    .color(theme::TEAL),
            ),
            Err(e) => ui.colored_label(theme::ERROR, e),
        };
    }

    /// Rückfrage, wenn Zieldateien schon existieren.
    fn ui_confirm(&mut self, ctx: &egui::Context) {
        let Some(list) = &self.confirm else {
            return;
        };
        let l = self.lang;
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("overwrite")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.label(
                egui::RichText::new(l.t("Dateien überschreiben?", "Overwrite files?"))
                    .heading()
                    .strong()
                    .color(theme::ACCENT),
            );
            ui.label(format!(
                "{} {}",
                list.len(),
                l.t(
                    "Zieldatei(en) gibt es schon:",
                    "output file(s) already exist:"
                )
            ));
            for p in list.iter().take(8) {
                ui.monospace(
                    p.file_name()
                        .map_or(String::new(), |n| n.to_string_lossy().into()),
                );
            }
            if list.len() > 8 {
                ui.label(format!("… {} {}", l.t("und", "and"), list.len() - 8));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(l.t("Überschreiben", "Overwrite"))
                                .color(egui::Color32::WHITE),
                        )
                        .fill(theme::ACCENT),
                    )
                    .clicked()
                {
                    choice = Some(Some(Existing::Overwrite));
                }
                if ui
                    .button(l.t("Vorhandene überspringen", "Skip existing"))
                    .clicked()
                {
                    choice = Some(Some(Existing::Skip));
                }
                if ui.button(l.t("Abbrechen", "Cancel")).clicked() {
                    choice = Some(None);
                }
            });
        });
        match choice {
            Some(Some(e)) => self.launch(ctx, e),
            Some(None) => self.confirm = None,
            None => {}
        }
    }

    fn ui_presets(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        ui.horizontal_wrapped(|ui| {
            ui.label("Preset");
            let mut chosen = None;
            egui::ComboBox::from_id_salt("preset")
                .selected_text(&self.preset_label)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for name in PRESET_NAMES {
                        if ui
                            .selectable_label(self.preset_label == name, name)
                            .on_hover_text(l.preset(name))
                            .clicked()
                        {
                            chosen = Some(name);
                        }
                    }
                });
            if let Some(name) = chosen {
                self.settings = Settings::preset(name).expect("eingebautes Preset");
                self.base = self.settings.clone();
                self.preset_label = name.to_string();
            }
            if ui.button(l.t("Laden…", "Load…")).clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Preset", &["toml"])
                    .pick_file()
            {
                self.load_preset(&path);
            }
            if ui.button(l.t("Speichern…", "Save…")).clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Preset", &["toml"])
                    .set_file_name(format!("{}.toml", self.preset_label))
                    .save_file()
            {
                let label = path.file_stem().map_or(self.preset_label.clone(), |s| {
                    s.to_string_lossy().into_owned()
                });
                let text = annotate_preset(&self.settings.to_preset_toml(&label));
                self.message = Some(match std::fs::write(&path, text) {
                    Ok(()) => (
                        false,
                        format!(
                            "{}: {}",
                            l.t("Preset gespeichert", "Preset saved"),
                            path.display()
                        ),
                    ),
                    Err(e) => (true, format!("{}: {e}", path.display())),
                });
            }
            if ui
                .button(l.t("Zurücksetzen", "Reset"))
                .on_hover_text(l.t(
                    "Alle Werte auf das gewählte bzw. geladene Preset zurücksetzen",
                    "Reset all values to the selected or loaded preset",
                ))
                .clicked()
            {
                self.settings = self.base.clone();
            }
        });
        ui.label(egui::RichText::new(l.preset(&self.preset_label)).weak());
    }

    fn load_preset(&mut self, path: &Path) {
        let r = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| Settings::from_preset_toml(&t))
            .and_then(|s| s.validate().map(|_| s));
        match r {
            Ok(s) => {
                self.settings = s.clone();
                self.base = s;
                self.preset_label = path
                    .file_stem()
                    .map_or("preset".into(), |s| s.to_string_lossy().into_owned());
                self.message = Some((
                    false,
                    format!(
                        "{}: {}",
                        self.lang.t("Preset geladen", "Preset loaded"),
                        path.display()
                    ),
                ));
            }
            Err(e) => self.message = Some((true, format!("{}: {e}", path.display()))),
        }
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        let table = settings_table(&self.settings);
        let mut changes: Vec<(&'static str, toml::Value)> = Vec::new();
        for group in PARAM_GROUPS {
            let params: Vec<&ParamSpec> = PARAMS.iter().filter(|p| p.group == group).collect();
            egui::CollapsingHeader::new(l.group(group))
                .id_salt(group)
                .default_open(false)
                .show(ui, |ui| {
                    let grid =
                        |ui: &mut egui::Ui, id: &str, primary: bool, changes: &mut Vec<_>| {
                            egui::Grid::new(id)
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    for p in params.iter().filter(|p| p.primary == primary) {
                                        if let Some(v) = lookup(&table, p.key) {
                                            let (label, help) = l.param(p);
                                            ui.label(label).on_hover_text(help);
                                            if let Some(nv) = param_widget(ui, l, p, v) {
                                                changes.push((p.key, nv));
                                            }
                                            ui.end_row();
                                        }
                                    }
                                });
                        };
                    grid(ui, &format!("{group}-p"), true, &mut changes);
                    if params.iter().any(|p| !p.primary) {
                        egui::CollapsingHeader::new(l.t("Feineinstellungen", "Fine tuning"))
                            .id_salt(format!("{group}-fein"))
                            .show(ui, |ui| {
                                grid(ui, &format!("{group}-f"), false, &mut changes)
                            });
                    }
                });
        }
        for (k, v) in changes {
            if let Err(e) = self.settings.set_value(k, v) {
                self.message = Some((true, e));
            }
        }
        if let Err(e) = self.settings.validate() {
            ui.colored_label(
                theme::ERROR,
                format!("{}: {e}", l.t("Ungültige Einstellung", "Invalid setting")),
            );
        }
    }

    fn ui_run(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let l = self.lang;
        if let Some(r) = &self.running {
            let st = r.progress.state.lock().expect("Mutex");
            let total = r.names.len();
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().color(theme::ACCENT));
                let (text, color) = if r.cancel.is_cancelled() {
                    (
                        l.t("Status: wird abgebrochen …", "Status: cancelling …")
                            .to_string(),
                        theme::GOLD,
                    )
                } else {
                    (
                        match l {
                            Lang::De => format!(
                                "Status: in Arbeit – {} von {total} Dateien fertig ({:.0} s)",
                                st.done,
                                r.started.elapsed().as_secs_f64()
                            ),
                            Lang::En => format!(
                                "Status: working – {} of {total} files done ({:.0} s)",
                                st.done,
                                r.started.elapsed().as_secs_f64()
                            ),
                        },
                        theme::ACCENT,
                    )
                };
                ui.label(egui::RichText::new(text).strong().color(color));
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(st.done as f32 / total.max(1) as f32)
                        .text(format!(
                            "{} {} {total} {}",
                            st.done,
                            l.t("von", "of"),
                            l.t("Dateien", "files")
                        ))
                        .fill(theme::ACCENT)
                        .desired_width(320.0),
                );
                if r.cancel.is_cancelled() {
                    ui.label(l.t("wird abgebrochen …", "cancelling …"));
                } else if ui.button(l.t("Abbrechen", "Cancel")).clicked() {
                    r.cancel.cancel();
                }
            });
            for (id, (done, all, pass)) in &st.files {
                let frac = if *all > 0 {
                    *done as f32 / *all as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(frac.min(1.0))
                        .text(format!("{}: {}", l.pass(pass), r.names[*id]))
                        .fill(theme::TEAL)
                        .desired_width(ui.available_width().min(600.0)),
                );
            }
        } else {
            let ok = self.settings.validate().is_ok() && matches!(self.wav_count, Ok(n) if n > 0);
            ui.horizontal_wrapped(|ui| {
                let enabled = ok && self.confirm.is_none();
                let label = egui::RichText::new(l.t("▶ Exportieren", "▶ Export"))
                    .strong()
                    .size(15.0);
                let button = if enabled {
                    egui::Button::new(label.color(egui::Color32::WHITE)).fill(theme::ACCENT)
                } else {
                    egui::Button::new(label)
                };
                if ui
                    .add_enabled(
                        enabled,
                        button
                            .min_size([130.0, 30.0].into())
                            .corner_radius(egui::CornerRadius::same(8)),
                    )
                    .on_hover_text(format!(
                        "{}: Name{}{}.wav",
                        l.t(
                            "Schreibt neben jede Originaldatei",
                            "Writes next to each original file"
                        ),
                        job::SUFFIX,
                        self.preset_label
                    ))
                    .clicked()
                {
                    self.start(ctx);
                }
                ui.label(l.t("Parallele Dateien", "Parallel files"));
                ui.add(egui::Slider::new(&mut self.jobs, 1..=16));
                let weak = ui.visuals().weak_text_color();
                let (text, color) = match &self.finished {
                    _ if !ok => (
                        l.t(
                            "Status: bereit, sobald WAV-Dateien hinzugefügt sind",
                            "Status: ready once WAV files are added",
                        )
                        .to_string(),
                        weak,
                    ),
                    None => (
                        l.t("Status: bereit", "Status: ready").to_string(),
                        theme::TEAL,
                    ),
                    Some(f) if f.cancelled => (
                        l.t("Status: abgebrochen", "Status: cancelled").to_string(),
                        theme::GOLD,
                    ),
                    Some(f) => {
                        let err = f.outcome.files.iter().filter(|o| o.is_error()).count();
                        if err > 0 {
                            (
                                match l {
                                    Lang::De => format!("Status: fertig, {err} Fehler"),
                                    Lang::En => format!("Status: done, {err} error(s)"),
                                },
                                theme::ERROR,
                            )
                        } else {
                            (
                                l.t("Status: fertig ✔", "Status: done ✔").to_string(),
                                theme::OK,
                            )
                        }
                    }
                };
                ui.label(egui::RichText::new(text).strong().color(color));
            });
        }
        if let Some((err, m)) = &self.message {
            let color = if *err { theme::ERROR } else { theme::OK };
            ui.colored_label(color, m);
        }
    }

    fn ui_result(&mut self, ui: &mut egui::Ui) {
        let Some(f) = &self.finished else {
            return;
        };
        let ok = f
            .outcome
            .files
            .iter()
            .filter(|o| matches!(o.status, Status::Ok { .. }))
            .count();
        let err = f.outcome.files.iter().filter(|o| o.is_error()).count();
        let l = self.lang;
        ui.label(format!(
            "{}{ok} ok, {err} {}, {} {} – {:.1} s",
            if f.cancelled {
                l.t("Abgebrochen. ", "Cancelled. ")
            } else {
                ""
            },
            l.t("Fehler", "error(s)"),
            f.outcome.files.len(),
            l.t("gesamt", "total"),
            f.seconds
        ));
        if let Some(a) = &f.outcome.album {
            ui.label(format!(
                "Album: {} {} dB, {} {} LUFS",
                l.t("gemeinsamer Gain", "shared gain"),
                a.gain_db.map_or("–".into(), |g| format!("{g:+.1}")),
                l.t("Ausgabe", "output"),
                a.loudness_out_lufs
                    .map_or("–".into(), |l| format!("{l:.1}"))
            ));
        }
        result_table(ui, l, &f.outcome.files);
        let notes = notes(l, &f.outcome);
        if !notes.is_empty() {
            egui::CollapsingHeader::new(format!("{} ({})", l.t("Hinweise", "Notes"), notes.len()))
                .id_salt("notes")
                .default_open(err > 0)
                .show(ui, |ui| {
                    for n in notes {
                        ui.label(n);
                    }
                });
        }
        let mut msg = None;
        ui.horizontal(|ui| {
            if ui
                .button(l.t("JSON-Report speichern…", "Save JSON report…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .set_file_name("report.json")
                    .save_file()
            {
                let meta = JsonMeta {
                    command: "process",
                    settings: &f.settings,
                    album: f.outcome.album.as_ref(),
                };
                msg = Some(match write_json(&path, &f.outcome.files, meta) {
                    Ok(()) => (
                        false,
                        format!(
                            "{}: {}",
                            l.t("Report gespeichert", "Report saved"),
                            path.display()
                        ),
                    ),
                    Err(e) => (true, format!("{e:#}")),
                });
            }
            if let Some(dir) = &f.folder
                && ui.button(l.t("Ordner öffnen", "Open folder")).clicked()
            {
                open_folder(dir);
            }
        });
        if msg.is_some() {
            self.message = msg;
        }
    }
}

/// Bedienelement für einen Parameter; liefert den neuen Wert bei Änderung.
fn param_widget(ui: &mut egui::Ui, l: Lang, p: &ParamSpec, v: &toml::Value) -> Option<toml::Value> {
    let help = l.param(p).1;
    match p.kind {
        ParamKind::Bool => {
            let mut b = v.as_bool().unwrap_or(false);
            ui.checkbox(&mut b, "")
                .on_hover_text(help)
                .changed()
                .then_some(b.into())
        }
        ParamKind::Float {
            min,
            max,
            unit,
            decimals,
            percent,
        } => {
            let x = v
                .as_float()
                .or_else(|| v.as_integer().map(|i| i as f64))
                .unwrap_or(min);
            let scale = if percent { 100.0 } else { 1.0 };
            let mut shown = x * scale;
            let r = ui
                .add(
                    egui::Slider::new(&mut shown, min * scale..=max * scale)
                        .suffix(format!(" {unit}"))
                        .fixed_decimals(decimals)
                        .clamping(egui::SliderClamping::Always),
                )
                .on_hover_text(help);
            r.changed().then(|| {
                let f = 10f64.powi(decimals as i32);
                ((shown * f).round() / f / scale).into()
            })
        }
        ParamKind::Choice(options) => {
            let cur = v.as_str().unwrap_or_default().to_string();
            let mut out = None;
            let text = options
                .iter()
                .find(|(k, _)| *k == cur)
                .map_or(cur.clone(), |(_, label)| l.choice(label).to_string());
            egui::ComboBox::from_id_salt(p.key)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for (k, label) in options {
                        if ui.selectable_label(cur == *k, l.choice(label)).clicked() && cur != *k {
                            out = Some((*k).into());
                        }
                    }
                })
                .response
                .on_hover_text(help);
            out
        }
        ParamKind::IntChoice(options) => {
            let cur = v.as_integer().unwrap_or_default();
            let mut out = None;
            egui::ComboBox::from_id_salt(p.key)
                .selected_text(format!("{cur}×"))
                .show_ui(ui, |ui| {
                    for &o in options {
                        if ui.selectable_label(cur == o, format!("{o}×")).clicked() && cur != o {
                            out = Some(o.into());
                        }
                    }
                })
                .response
                .on_hover_text(help);
            out
        }
        ParamKind::FftSize => {
            let cur = v.as_integer().map_or("auto".to_string(), |n| n.to_string());
            let mut out = None;
            egui::ComboBox::from_id_salt(p.key)
                .selected_text(&cur)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(cur == "auto", "auto").clicked() && cur != "auto" {
                        out = Some("auto".into());
                    }
                    for n in [512i64, 1024, 2048, 4096, 8192] {
                        let s = n.to_string();
                        if ui.selectable_label(cur == s, &s).clicked() && cur != s {
                            out = Some(n.into());
                        }
                    }
                })
                .response
                .on_hover_text(help);
            out
        }
    }
}

fn fmt(v: Option<f64>) -> String {
    v.map_or("–".into(), |v| format!("{v:.1}"))
}

fn result_table(ui: &mut egui::Ui, l: Lang, files: &[FileOutcome]) {
    egui::ScrollArea::horizontal()
        .id_salt("result")
        .show(ui, |ui| {
            egui::Grid::new("result-grid")
                .striped(true)
                .num_columns(11)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    let heads: [(&str, &str, &str); 11] = [
                        (l.t("Datei", "File"), "", ""),
                        ("Status", "", ""),
                        (l.t("Ein LUFS", "In LUFS"), "", ""),
                        (l.t("Ein dBTP", "In dBTP"), "", ""),
                        (
                            l.t("Kn", "Kn"),
                            "erkannte Knackser/Griffgeräusche",
                            "detected knocks/handling noise",
                        ),
                        (
                            "Leveler",
                            "Gain-Bereich des Levelers in Sprache (dB)",
                            "leveler gain range in speech (dB)",
                        ),
                        (l.t("Aus LUFS", "Out LUFS"), "", ""),
                        (l.t("Aus dBTP", "Out dBTP"), "", ""),
                        (
                            "Gain",
                            "Normalisierungs-Gain (dB)",
                            "normalization gain (dB)",
                        ),
                        (
                            l.t("Kmp", "Cmp"),
                            "größte Absenkung durch den Kompressor (dB)",
                            "largest compressor gain reduction (dB)",
                        ),
                        (
                            "Lim",
                            "größte Absenkung durch den Limiter (dB)",
                            "largest limiter gain reduction (dB)",
                        ),
                    ];
                    for (h, de, en) in heads {
                        let r = ui.strong(h);
                        if !de.is_empty() {
                            r.on_hover_text(if l == Lang::De { de } else { en });
                        }
                    }
                    ui.end_row();
                    for o in files {
                        ui.label(&o.display);
                        match &o.status {
                            Status::Ok { report: r } => {
                                ui.label("ok");
                                ui.label(fmt(r.loudness_in.integrated_lufs));
                                ui.label(fmt(r.loudness_in.true_peak_dbtp));
                                ui.label(
                                    r.knock
                                        .as_ref()
                                        .map_or("–".into(), |k| k.regions.to_string()),
                                );
                                ui.label(r.leveler.as_ref().map_or("–".into(), |l| {
                                    if l.phrases == 0 {
                                        "–".into()
                                    } else {
                                        format!("{:+.0}…{:+.0}", l.gain_min_db, l.gain_max_db)
                                    }
                                }));
                                let out = r.loudness_out;
                                ui.label(fmt(out.and_then(|o| o.integrated_lufs)));
                                ui.label(fmt(out.and_then(|o| o.true_peak_dbtp)));
                                ui.label(
                                    r.normalization_gain_db
                                        .map_or("–".into(), |g| format!("{g:+.1}")),
                                );
                                ui.label(fmt(r
                                    .compressor
                                    .as_ref()
                                    .map(|c| c.max_gain_reduction_db)));
                                ui.label(fmt(r.limiter.as_ref().map(|l| l.max_gain_reduction_db)));
                            }
                            Status::Skipped { .. } => {
                                ui.label(l.t("übersprungen", "skipped"));
                            }
                            Status::Cancelled => {
                                ui.label(l.t("abgebrochen", "cancelled"));
                            }
                            Status::Error { .. } => {
                                ui.colored_label(theme::ERROR, l.t("FEHLER", "ERROR"));
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}

fn notes(l: Lang, o: &Outcome) -> Vec<String> {
    let mut n = Vec::new();
    if let Some(a) = &o.album {
        n.extend(a.warnings.iter().map(|w| format!("Album: {w}")));
    }
    for f in &o.files {
        match &f.status {
            Status::Ok { report } => {
                n.extend(
                    report
                        .warnings
                        .iter()
                        .map(|w| format!("{}: {w}", f.display)),
                );
                if let Some(g) = &report.gate {
                    n.push(match l {
                        Lang::De => format!(
                            "{}: Pausen um {:.1} dB abgesenkt (Ziel {:.0} dBFS)",
                            f.display, g.attenuation_db, g.floor_dbfs
                        ),
                        Lang::En => format!(
                            "{}: pauses lowered by {:.1} dB (target {:.0} dBFS)",
                            f.display, g.attenuation_db, g.floor_dbfs
                        ),
                    });
                }
            }
            Status::Error { message } => n.push(format!("{}: {message}", f.display)),
            _ => {}
        }
    }
    n
}

fn open_folder(dir: &Path) {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() && self.running.is_none() {
            self.add_inputs(dropped);
        }
        let busy = self.running.is_some();
        let bar = egui::Frame::side_top_panel(ui.style())
            .fill(theme::BAR)
            .stroke(egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.35)));
        egui::Panel::bottom("run")
            .min_size(56.0)
            .frame(bar)
            .show(ui, |ui| {
                ui.add_space(6.0);
                self.ui_run(ui, &ctx);
                ui.add_space(6.0);
            });
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(APP_NAME)
                        .heading()
                        .strong()
                        .color(theme::ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (lang, label, tip) in
                        [(Lang::De, "DE", "Deutsch"), (Lang::En, "EN", "English")]
                    {
                        if ui
                            .selectable_label(self.lang == lang, label)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            self.lang = lang;
                        }
                    }
                    ui.label(egui::RichText::new("🌐").color(theme::ACCENT));
                });
            });
        });
        self.ui_confirm(&ctx);
        let l = self.lang;
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    section(ui, "1", l.t("Eingabe", "Input"));
                    self.ui_input(ui);
                    ui.separator();
                    section(ui, "2", l.t("Einstellungen", "Settings"));
                    self.ui_presets(ui);
                    ui.add_space(4.0);
                    self.ui_settings(ui);
                });
                if self.finished.is_some() {
                    ui.separator();
                    section(ui, "3", l.t("Ergebnis", "Result"));
                    self.ui_result(ui);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App as _;

    fn collect(s: &egui::Shape, v: &mut Vec<(String, egui::Pos2)>) {
        match s {
            egui::Shape::Text(t) => v.push((t.galley.text().to_string(), t.pos)),
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, v)),
            _ => {}
        }
    }

    /// Einen Frame rendern; liefert alle Texte mit Position.
    fn frame(
        ctx: &egui::Context,
        app: &mut App,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let mut f = eframe::Frame::_new_kittest();
        let mut out = ctx.run_ui(input, |ui| app.ui(ui, &mut f));
        // Kein Renderer im Test: Textur-Updates verwerfen.
        out.textures_delta.clear();
        let mut v = Vec::new();
        for cs in &out.shapes {
            collect(&cs.shape, &mut v);
        }
        v
    }

    #[test]
    fn export_click_shows_status_and_hides_button() {
        let dir = leveled_audio_testkit::temp_dir("gui-click");
        let wav = dir.join("a.wav");
        leveled_audio_testkit::write_wav(
            &wav,
            48_000,
            leveled_audio_io::SampleFormat::Int24,
            &leveled_audio_testkit::speechlike(48_000, 2, -28.0, 30.0, 4),
        );
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.lang = Lang::De;
        app.add_inputs([wav.clone()]);
        frame(&ctx, &mut app, vec![]);
        let t = frame(&ctx, &mut app, vec![]);
        let (_, pos) = t
            .iter()
            .find(|(s, _)| s.contains("Exportieren"))
            .expect("Button")
            .clone();
        let p = pos + egui::vec2(5.0, 5.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        frame(&ctx, &mut app, vec![press(true)]);
        frame(&ctx, &mut app, vec![press(false)]);
        assert!(app.running.is_some(), "nicht gestartet: {:?}", app.message);
        let t = frame(&ctx, &mut app, vec![]);
        let all: Vec<&str> = t.iter().map(|(s, _)| s.as_str()).collect();
        assert!(all.iter().any(|s| s.contains("in Arbeit")), "{all:?}");
        assert!(!all.iter().any(|s| s.contains("Exportieren")), "{all:?}");
        let t0 = Instant::now();
        while app.finished.is_none() && t0.elapsed().as_secs() < 60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            frame(&ctx, &mut app, vec![]);
        }
        let t = frame(&ctx, &mut app, vec![]);
        let all: Vec<&str> = t.iter().map(|(s, _)| s.as_str()).collect();
        assert!(all.iter().any(|s| s.contains("Status: fertig")), "{all:?}");
        assert!(dir.join("a-leveled-podcast-stereo.wav").exists());

        // Zweiter Export: Rückfrage, „Überschreiben“ startet.
        let click = |app: &mut App, label: &str| {
            let t = frame(&ctx, app, vec![]);
            let (_, pos) = t
                .iter()
                .find(|(s, _)| s.contains(label))
                .unwrap_or_else(|| panic!("{label} fehlt"))
                .clone();
            let p = pos + egui::vec2(5.0, 5.0);
            let press = |pressed| egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(&ctx, app, vec![egui::Event::PointerMoved(p)]);
            frame(&ctx, app, vec![press(true)]);
            frame(&ctx, app, vec![press(false)]);
        };
        click(&mut app, "Exportieren");
        assert!(app.running.is_none());
        let t = frame(&ctx, &mut app, vec![]);
        assert!(t.iter().any(|(s, _)| s.contains("überschreiben?")));
        click(&mut app, "Überschreiben");
        assert!(app.confirm.is_none() && app.running.is_some());
        app.running.as_ref().unwrap().cancel.cancel();
        while app.finished.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame(&ctx, &mut app, vec![]);
        }
    }

    #[test]
    fn starts_in_english_and_switches_to_german() {
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        assert_eq!(app.lang, Lang::En);
        frame(&ctx, &mut app, vec![]);
        let t = frame(&ctx, &mut app, vec![]);
        let has = |t: &[(String, egui::Pos2)], s: &str| t.iter().any(|(x, _)| x == s);
        assert!(has(&t, "Input") && has(&t, "▶ Export"), "{t:?}");
        assert!(!has(&t, "Eingabe"));
        let (_, pos) = t.iter().find(|(s, _)| s == "DE").expect("DE").clone();
        let p = pos + egui::vec2(3.0, 3.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        frame(&ctx, &mut app, vec![press(true)]);
        frame(&ctx, &mut app, vec![press(false)]);
        assert_eq!(app.lang, Lang::De);
        let t = frame(&ctx, &mut app, vec![]);
        assert!(has(&t, "Eingabe") && has(&t, "▶ Exportieren"), "{t:?}");
    }
}
