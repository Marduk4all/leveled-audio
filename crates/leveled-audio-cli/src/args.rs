use clap::{Args, Parser, Subcommand, ValueEnum};
use leveled_audio_pipeline::BitDepth;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "leveled-audio",
    version,
    about = "Batch-Verarbeitung von Sprachaufnahmen (WAV): Lautheit normalisieren, True-Peak-Limiter",
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
    Process(ProcessArgs),
    /// Dateien nur messen (Lautheit, True Peak, LRA), nichts schreiben
    Analyze(AnalyzeArgs),
    /// Eingebaute Presets anzeigen
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
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
    /// Preset-Name (podcast-stereo, podcast-mono, ebu-r128) oder Pfad zu einer .toml-Datei
    #[arg(long, default_value = "podcast-stereo")]
    pub preset: String,
    /// Ziel-Lautheit in LUFS (überschreibt das Preset)
    #[arg(long, allow_negative_numbers = true)]
    pub target_lufs: Option<f64>,
    /// True-Peak-Ceiling in dBTP (überschreibt das Preset)
    #[arg(long, allow_negative_numbers = true)]
    pub ceiling_dbtp: Option<f64>,
    /// Bittiefe der Ausgabe
    #[arg(long, value_enum)]
    pub bit_depth: Option<BitDepthArg>,
    /// Keine Loudness-Normalisierung
    #[arg(long)]
    pub no_normalize: bool,
    /// Keinen True-Peak-Limiter verwenden
    #[arg(long)]
    pub no_limiter: bool,
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
