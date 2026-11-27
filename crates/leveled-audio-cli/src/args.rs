use clap::{Args, Parser, Subcommand, ValueEnum};
use leveled_audio_pipeline::{BitDepth, GainRuleSetting, LoudnessMode};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "leveled-audio",
    version,
    about = "Batch-Verarbeitung von Sprachaufnahmen (WAV): Rauschreduktion, Lautheit, True-Peak-Limiter",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    /// Keine Fortschrittsanzeige und keine Zusammenfassung
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Dateien verarbeiten und in einen Ausgabeordner schreiben
    Process(Box<ProcessArgs>),
    /// Dateien nur messen (Lautheit, True Peak, LRA, Rauschpegel), nichts schreiben
    Analyze(AnalyzeArgs),
    /// Rauschprofile
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Eingebaute Presets anzeigen
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
    },
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// Rauschprofil aus einer Aufnahme lernen (z. B. Raumton) und als JSON speichern
    Learn {
        /// WAV-Datei
        input: PathBuf,
        /// Zieldatei (.json)
        #[arg(short = 'o', long)]
        output: PathBuf,
        /// Preset für die Analyseparameter
        #[arg(long, default_value = "podcast-stereo")]
        preset: String,
    },
}

#[derive(Subcommand)]
pub enum PresetCmd {
    /// Eingebaute Presets auflisten
    List,
    /// Preset als TOML ausgeben (Vorlage für eigene Presets)
    Show { name: String },
}

#[derive(Args)]
pub struct ProcessArgs {
    /// WAV-Dateien und/oder Ordner
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Ausgabeordner (wird bei Bedarf angelegt)
    #[arg(short = 'o', long = "output-dir")]
    pub output_dir: PathBuf,
    /// Preset: podcast-stereo (natürlich, Default), podcast-kraeftig (nahe Referenz), podcast-mono,
    /// ebu-r128 oder Pfad zu einer .toml-Datei (siehe `leveled-audio preset list`)
    #[arg(long, default_value = "podcast-stereo")]
    pub preset: String,
    /// Einzelne Einstellung überschreiben, z. B. --set denoise.reduction_db=15 (mehrfach möglich)
    #[arg(long = "set", value_name = "SCHLÜSSEL=WERT")]
    pub overrides: Vec<String>,
    /// Ziel-Lautheit in LUFS (Default −16). Höher = lauter; Plattformen normalisieren meist auf −14 bis −16
    #[arg(long, allow_negative_numbers = true, value_name = "LUFS")]
    pub target_lufs: Option<f64>,
    /// Lautheit pro Datei oder gemeinsamer Gain für alle Dateien (Album)
    #[arg(long, value_enum)]
    pub loudness_mode: Option<LoudnessModeArg>,
    /// Obergrenze für Spitzen in dBTP (Default −1). Höher entlastet den Limiter, riskiert aber Verzerrungen nach MP3/AAC
    #[arg(long, allow_negative_numbers = true, value_name = "DBTP")]
    pub ceiling_dbtp: Option<f64>,
    /// Hochpass in Hz (Default 70): entfernt Rumpeln und Trittschall. Ab ≈ 100 Hz verlieren tiefe Stimmen Wärme
    #[arg(long, value_name = "HZ")]
    pub highpass: Option<f64>,
    /// Keinen Hochpass
    #[arg(long, conflicts_with = "highpass")]
    pub no_highpass: bool,
    /// Absenkung der Tiefen in erkannten Griffgeräuschen/Knacksern in dB (Default 15; 0 = nur erkennen).
    /// Höher = leiser; gleichzeitige Sprache klingt dort kurz dünner
    #[arg(long, value_name = "DB")]
    pub knock_reduction: Option<f64>,
    /// Keine Störgeräusch-Erkennung (Knackser zählen dann als Sprache und werden mitgelevelt)
    #[arg(long, conflicts_with = "knock_reduction")]
    pub no_knock: bool,
    /// Kompressor in Prozent (Default 40 ≈ 1,5:1; 60 ≈ 2:1): fängt laute Stellen vor dem Limiter ab.
    /// Höher = dichter, zu hoch klingt gepresst. 0 = aus
    #[arg(long, value_name = "PROZENT")]
    pub compression: Option<f64>,
    /// Keinen Kompressor
    #[arg(long, conflicts_with = "compression")]
    pub no_compression: bool,
    /// Pausen-Absenkung: Pegel der Sprechpausen in der fertigen Datei in dBFS, z. B. −60 (Default aus).
    /// Pausen werden fast still; an Pausengrenzen kann das Rauschen hörbar „atmen“
    #[arg(long, allow_negative_numbers = true, value_name = "DBFS")]
    pub pause_floor: Option<f64>,
    /// Keine Pausen-Absenkung (natürlicher Raumton bleibt)
    #[arg(long, conflicts_with = "pause_floor")]
    pub no_pause_floor: bool,
    /// Bittiefe der Ausgabe
    #[arg(long, value_enum)]
    pub bit_depth: Option<BitDepthArg>,
    /// Keine Loudness-Normalisierung
    #[arg(long)]
    pub no_normalize: bool,
    /// Keinen True-Peak-Limiter verwenden
    #[arg(long)]
    pub no_limiter: bool,
    /// Keine Rauschreduktion
    #[arg(long)]
    pub no_denoise: bool,
    /// Maximale Rauschabsenkung in dB (Default 12). Höher = weniger Rauschen; ab ≈ 15–18 dB drohen
    /// „blubbernde“ Artefakte und dumpfere Sprache
    #[arg(long, value_name = "DB")]
    pub reduction_db: Option<f64>,
    /// Gain-Regel der Rauschreduktion
    #[arg(long, value_enum)]
    pub gain_rule: Option<GainRuleArg>,
    /// Keinen Leveler (Angleichen unterschiedlich lauter Sprecher)
    #[arg(long)]
    pub no_leveler: bool,
    /// Leveler in Prozent, 0–130 (Default 130): gleicht leise und laute Sprecher an. 100 = gleich
    /// laut, 0 = Unterschiede bleiben, über 100 = überkorrigieren. Höher klingt gleichmäßiger,
    /// aber weniger lebendig
    #[arg(long, value_name = "PROZENT")]
    pub leveler_strength: Option<f64>,
    /// Dieses Rauschprofil (JSON) für alle Dateien verwenden
    #[arg(long, value_name = "DATEI")]
    pub noise_profile: Option<PathBuf>,
    /// Automatisch bestimmte Rauschprofile in diesem Ordner speichern
    #[arg(long, value_name = "ORDNER")]
    pub save_profiles: Option<PathBuf>,
    /// Entfernte Anteile zusätzlich als <name>.delta.wav schreiben (Kontrolle)
    #[arg(long)]
    pub export_delta: bool,
    /// Benennungsschema; Platzhalter: {stem} {ext} {preset} {lufs}
    #[arg(long, default_value = "{stem}{ext}")]
    pub naming: String,
    /// Vorhandene Zieldateien überschreiben (Eingabedateien nie)
    #[arg(long, conflicts_with = "skip_existing")]
    pub overwrite: bool,
    /// Dateien überspringen, deren Ziel schon existiert
    #[arg(long)]
    pub skip_existing: bool,
    /// Ordner rekursiv durchsuchen (Unterordner-Struktur bleibt erhalten)
    #[arg(short, long)]
    pub recursive: bool,
    /// Anzahl paralleler Dateien (Default: Kerne, höchstens 4)
    #[arg(short, long)]
    pub jobs: Option<usize>,
    /// Report zusätzlich als JSON schreiben
    #[arg(long)]
    pub report_json: Option<PathBuf>,
}

#[derive(Args)]
pub struct AnalyzeArgs {
    /// WAV-Dateien und/oder Ordner
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Preset für die Analyseparameter
    #[arg(long, default_value = "podcast-stereo")]
    pub preset: String,
    /// Einzelne Einstellung überschreiben (wie bei process)
    #[arg(long = "set", value_name = "SCHLÜSSEL=WERT")]
    pub overrides: Vec<String>,
    /// Ordner rekursiv durchsuchen
    #[arg(short, long)]
    pub recursive: bool,
    /// Anzahl paralleler Dateien
    #[arg(short, long)]
    pub jobs: Option<usize>,
    /// Report zusätzlich als JSON schreiben
    #[arg(long)]
    pub json: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum BitDepthArg {
    /// Wie die Eingabe (32-bit int → 24-bit)
    Same,
    #[value(name = "16")]
    I16,
    #[value(name = "24")]
    I24,
    #[value(name = "32f")]
    F32,
}

impl From<BitDepthArg> for BitDepth {
    fn from(b: BitDepthArg) -> Self {
        match b {
            BitDepthArg::Same => BitDepth::Same,
            BitDepthArg::I16 => BitDepth::I16,
            BitDepthArg::I24 => BitDepth::I24,
            BitDepthArg::F32 => BitDepth::F32,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum GainRuleArg {
    /// Wiener-Filter mit Decision-Directed-Schätzung (weniger Musical Noise)
    WienerDd,
    /// Klassische Power Subtraction
    Subtract,
}

impl From<GainRuleArg> for GainRuleSetting {
    fn from(g: GainRuleArg) -> Self {
        match g {
            GainRuleArg::WienerDd => GainRuleSetting::WienerDd,
            GainRuleArg::Subtract => GainRuleSetting::Subtract,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum LoudnessModeArg {
    /// Jede Datei trifft das Ziel
    PerFile,
    /// Ein Gain für alle Dateien; die gemeinsame Lautheit trifft das Ziel
    Album,
}

impl From<LoudnessModeArg> for LoudnessMode {
    fn from(m: LoudnessModeArg) -> Self {
        match m {
            LoudnessModeArg::PerFile => LoudnessMode::PerFile,
            LoudnessModeArg::Album => LoudnessMode::Album,
        }
    }
}
