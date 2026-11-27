//! Oberfläche (egui): ein Fenster mit Eingabe, Einstellungen, Verarbeitung
//! und Ergebnis (PLAN Phase 6). Exportiert wird neben die Originaldateien
//! (`<Name>-leveled-<Preset>.wav`); vorhandene Ziele nur nach Rückfrage.

use crate::job::{self, Existing, Outcome, Request};
use eframe::egui;
use leveled_audio_cli::summary::{FileOutcome, JsonMeta, Status, annotate_preset, write_json};
use leveled_audio_pipeline::{
    CancelToken, PARAM_GROUPS, PARAMS, PRESET_NAMES, ParamKind, ParamSpec, Pass, ProgressSink,
    Settings, lookup, preset_description, settings_table,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let settings = Settings::default();
        App {
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
        ui.horizontal(|ui| {
            if ui.button("Dateien hinzufügen…").clicked()
                && let Some(files) = rfd::FileDialog::new()
                    .add_filter("WAV", &["wav", "WAV"])
                    .pick_files()
            {
                self.add_inputs(files);
            }
            if ui.button("Ordner hinzufügen…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.add_inputs([dir]);
            }
            if ui
                .checkbox(&mut self.recursive, "Unterordner einbeziehen")
                .changed()
            {
                self.recount();
            }
            if !self.inputs.is_empty() && ui.button("Alle entfernen").clicked() {
                self.inputs.clear();
                self.recount();
            }
        });
        ui.label(
            egui::RichText::new("Dateien oder Ordner lassen sich auch ins Fenster ziehen.").weak(),
        );
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("inputs")
            .max_height(120.0)
            .show(ui, |ui| {
                for (i, p) in self.inputs.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✕").on_hover_text("Entfernen").clicked() {
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
            Ok(n) => ui.label(format!("{n} WAV-Datei(en)")),
            Err(e) => ui.colored_label(egui::Color32::DARK_RED, e),
        };
    }

    /// Rückfrage, wenn Zieldateien schon existieren.
    fn ui_confirm(&mut self, ctx: &egui::Context) {
        let Some(list) = &self.confirm else {
            return;
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("overwrite")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading("Dateien überschreiben?");
            ui.label(format!("{} Zieldatei(en) gibt es schon:", list.len()));
            for p in list.iter().take(8) {
                ui.monospace(
                    p.file_name()
                        .map_or(String::new(), |n| n.to_string_lossy().into()),
                );
            }
            if list.len() > 8 {
                ui.label(format!("… und {} weitere", list.len() - 8));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Überschreiben").clicked() {
                    choice = Some(Some(Existing::Overwrite));
                }
                if ui.button("Vorhandene überspringen").clicked() {
                    choice = Some(Some(Existing::Skip));
                }
                if ui.button("Abbrechen").clicked() {
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
        ui.horizontal(|ui| {
            ui.label("Preset");
            let mut chosen = None;
            egui::ComboBox::from_id_salt("preset")
                .selected_text(&self.preset_label)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for name in PRESET_NAMES {
                        if ui
                            .selectable_label(self.preset_label == name, name)
                            .on_hover_text(preset_description(name))
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
            if ui.button("Laden…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Preset", &["toml"])
                    .pick_file()
            {
                self.load_preset(&path);
            }
            if ui.button("Speichern…").clicked()
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
                    Ok(()) => (false, format!("Preset gespeichert: {}", path.display())),
                    Err(e) => (true, format!("{}: {e}", path.display())),
                });
            }
            if ui
                .button("Zurücksetzen")
                .on_hover_text("Alle Werte auf das gewählte bzw. geladene Preset zurücksetzen")
                .clicked()
            {
                self.settings = self.base.clone();
            }
        });
        ui.label(egui::RichText::new(preset_description(&self.preset_label)).weak());
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
                self.message = Some((false, format!("Preset geladen: {}", path.display())));
            }
            Err(e) => self.message = Some((true, format!("{}: {e}", path.display()))),
        }
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        let table = settings_table(&self.settings);
        let mut changes: Vec<(&'static str, toml::Value)> = Vec::new();
        for group in PARAM_GROUPS {
            let params: Vec<&ParamSpec> = PARAMS.iter().filter(|p| p.group == group).collect();
            egui::CollapsingHeader::new(group)
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
                                            ui.label(p.label).on_hover_text(p.help);
                                            if let Some(nv) = param_widget(ui, p, v) {
                                                changes.push((p.key, nv));
                                            }
                                            ui.end_row();
                                        }
                                    }
                                });
                        };
                    grid(ui, &format!("{group}-p"), true, &mut changes);
                    if params.iter().any(|p| !p.primary) {
                        egui::CollapsingHeader::new("Feineinstellungen")
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
                egui::Color32::DARK_RED,
                format!("Ungültige Einstellung: {e}"),
            );
        }
    }

    fn ui_run(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(r) = &self.running {
            let st = r.progress.state.lock().expect("Mutex");
            let total = r.names.len();
            ui.horizontal(|ui| {
                ui.spinner();
                ui.strong(if r.cancel.is_cancelled() {
                    "Status: wird abgebrochen …".to_string()
                } else {
                    format!(
                        "Status: in Arbeit – {} von {total} Dateien fertig ({:.0} s)",
                        st.done,
                        r.started.elapsed().as_secs_f64()
                    )
                });
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(st.done as f32 / total.max(1) as f32)
                        .text(format!("{} von {total} Dateien", st.done))
                        .desired_width(400.0),
                );
                if r.cancel.is_cancelled() {
                    ui.label("wird abgebrochen …");
                } else if ui.button("Abbrechen").clicked() {
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
                        .text(format!("{pass}: {}", r.names[*id]))
                        .desired_width(600.0),
                );
            }
        } else {
            let ok = self.settings.validate().is_ok() && matches!(self.wav_count, Ok(n) if n > 0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        ok && self.confirm.is_none(),
                        egui::Button::new("▶ Exportieren").min_size([120.0, 28.0].into()),
                    )
                    .on_hover_text(format!(
                        "Schreibt neben jede Originaldatei: Name{}{}.wav",
                        job::SUFFIX,
                        self.preset_label
                    ))
                    .clicked()
                {
                    self.start(ctx);
                }
                ui.label("Parallele Dateien");
                ui.add(egui::Slider::new(&mut self.jobs, 1..=16));
                ui.strong(match &self.finished {
                    _ if !ok => "Status: bereit, sobald WAV-Dateien hinzugefügt sind".to_string(),
                    None => "Status: bereit".to_string(),
                    Some(f) if f.cancelled => "Status: abgebrochen".to_string(),
                    Some(f) => {
                        let err = f.outcome.files.iter().filter(|o| o.is_error()).count();
                        if err > 0 {
                            format!("Status: fertig, {err} Fehler")
                        } else {
                            "Status: fertig".to_string()
                        }
                    }
                });
            });
        }
        if let Some((err, m)) = &self.message {
            let color = if *err {
                egui::Color32::DARK_RED
            } else {
                egui::Color32::DARK_GREEN
            };
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
        ui.label(format!(
            "{}{ok} ok, {err} Fehler, {} gesamt – {:.1} s",
            if f.cancelled { "Abgebrochen. " } else { "" },
            f.outcome.files.len(),
            f.seconds
        ));
        if let Some(a) = &f.outcome.album {
            ui.label(format!(
                "Album: gemeinsamer Gain {} dB, Ausgabe {} LUFS",
                a.gain_db.map_or("–".into(), |g| format!("{g:+.1}")),
                a.loudness_out_lufs
                    .map_or("–".into(), |l| format!("{l:.1}"))
            ));
        }
        result_table(ui, &f.outcome.files);
        let notes = notes(&f.outcome);
        if !notes.is_empty() {
            egui::CollapsingHeader::new(format!("Hinweise ({})", notes.len()))
                .default_open(err > 0)
                .show(ui, |ui| {
                    for n in notes {
                        ui.label(n);
                    }
                });
        }
        let mut msg = None;
        ui.horizontal(|ui| {
            if ui.button("JSON-Report speichern…").clicked()
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
                    Ok(()) => (false, format!("Report gespeichert: {}", path.display())),
                    Err(e) => (true, format!("{e:#}")),
                });
            }
            if let Some(dir) = &f.folder
                && ui.button("Ordner öffnen").clicked()
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
fn param_widget(ui: &mut egui::Ui, p: &ParamSpec, v: &toml::Value) -> Option<toml::Value> {
    match p.kind {
        ParamKind::Bool => {
            let mut b = v.as_bool().unwrap_or(false);
            ui.checkbox(&mut b, "")
                .on_hover_text(p.help)
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
                .on_hover_text(p.help);
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
                .map_or(cur.clone(), |(_, l)| l.to_string());
            egui::ComboBox::from_id_salt(p.key)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for (k, l) in options {
                        if ui.selectable_label(cur == *k, *l).clicked() && cur != *k {
                            out = Some((*k).into());
                        }
                    }
                })
                .response
                .on_hover_text(p.help);
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
                .on_hover_text(p.help);
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
                .on_hover_text(p.help);
            out
        }
    }
}

fn fmt(v: Option<f64>) -> String {
    v.map_or("–".into(), |v| format!("{v:.1}"))
}

fn result_table(ui: &mut egui::Ui, files: &[FileOutcome]) {
    egui::ScrollArea::horizontal()
        .id_salt("result")
        .show(ui, |ui| {
            egui::Grid::new("result-grid")
                .striped(true)
                .num_columns(11)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for h in [
                        "Datei", "Status", "Ein LUFS", "Ein dBTP", "Kn", "Leveler", "Aus LUFS",
                        "Aus dBTP", "Gain", "Kmp", "Lim",
                    ] {
                        ui.strong(h).on_hover_text(match h {
                            "Kn" => "erkannte Knackser/Griffgeräusche",
                            "Leveler" => "Gain-Bereich des Levelers in Sprache (dB)",
                            "Gain" => "Normalisierungs-Gain (dB)",
                            "Kmp" => "größte Absenkung durch den Kompressor (dB)",
                            "Lim" => "größte Absenkung durch den Limiter (dB)",
                            _ => "",
                        });
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
                                ui.label("übersprungen");
                            }
                            Status::Cancelled => {
                                ui.label("abgebrochen");
                            }
                            Status::Error { .. } => {
                                ui.colored_label(egui::Color32::DARK_RED, "FEHLER");
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}

fn notes(o: &Outcome) -> Vec<String> {
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
                    n.push(format!(
                        "{}: Pausen um {:.1} dB abgesenkt (Ziel {:.0} dBFS)",
                        f.display, g.attenuation_db, g.floor_dbfs
                    ));
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
        egui::Panel::bottom("run").min_size(60.0).show(ui, |ui| {
            ui.add_space(6.0);
            self.ui_run(ui, &ctx);
            ui.add_space(6.0);
        });
        self.ui_confirm(&ctx);
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.heading("1. Eingabe");
                    self.ui_input(ui);
                    ui.separator();
                    ui.heading("2. Einstellungen");
                    self.ui_presets(ui);
                    ui.add_space(4.0);
                    self.ui_settings(ui);
                });
                if self.finished.is_some() {
                    ui.separator();
                    ui.heading("3. Ergebnis");
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
        let out = ctx.run_ui(input, |ui| app.ui(ui, &mut f));
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
}
