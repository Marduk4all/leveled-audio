//! Audio-Ein-/Ausgabe für Leveled Audio.
//!
//! Im MVP nur WAV (RIFF und RF64/BW64). Weitere Formate docken später über
//! [`AudioSource`] / [`AudioSink`] an, ohne dass die Pipeline sich ändert.
//!
//! Konvention für Integer-Formate: `float = int / 2^(bits-1)`, beim Schreiben
//! umgekehrt mit Rundung und Begrenzung. Damit sind 16/24-bit-Roundtrips bitexakt.

mod wav;

pub use wav::{WavReader, WavWriter};

use std::path::Path;

/// Sampleformat einer Datei.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl SampleFormat {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::Int16 => 2,
            SampleFormat::Int24 => 3,
            SampleFormat::Int32 | SampleFormat::Float32 => 4,
        }
    }

    pub fn bits(self) -> u16 {
        (self.bytes_per_sample() * 8) as u16
    }

    pub fn is_float(self) -> bool {
        matches!(self, SampleFormat::Float32)
    }

    /// Kurzname für Reports, z. B. `"24-bit int"`.
    pub fn label(self) -> &'static str {
        match self {
            SampleFormat::Int16 => "16-bit int",
            SampleFormat::Int24 => "24-bit int",
            SampleFormat::Int32 => "32-bit int",
            SampleFormat::Float32 => "32-bit float",
        }
    }
}

/// Grunddaten eines Audiostroms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

impl AudioSpec {
    pub fn block_align(&self) -> usize {
        self.channels as usize * self.format.bytes_per_sample()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("E/A-Fehler: {0}")]
    Io(#[from] std::io::Error),
    #[error("keine WAV-Datei (RIFF/RF64-Kopf fehlt)")]
    NotWav,
    #[error("nicht unterstütztes Format: {0}")]
    Unsupported(String),
    #[error("defekte WAV-Datei: {0}")]
    Malformed(String),
    #[error("Ausgabe überschreitet 4 GiB und RF64 ist abgeschaltet")]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, IoError>;

/// Blockweise lesbare Audioquelle mit Seek (für Mehrpass-Verarbeitung).
pub trait AudioSource: Send {
    fn spec(&self) -> AudioSpec;
    fn total_frames(&self) -> u64;
    /// Liest bis zu `max_frames` Frames planar nach `out` (ein Vec pro Kanal,
    /// wird geleert und neu befüllt). Gibt die Anzahl gelesener Frames zurück,
    /// 0 am Dateiende.
    fn read(&mut self, out: &mut [Vec<f32>], max_frames: usize) -> Result<usize>;
    fn seek(&mut self, frame: u64) -> Result<()>;
}

/// Blockweise beschreibbare Audiosenke.
pub trait AudioSink: Send {
    fn spec(&self) -> AudioSpec;
    /// Schreibt `frames` Frames aus planaren Kanalpuffern.
    fn write(&mut self, channels: &[Vec<f32>], frames: usize) -> Result<()>;
    /// Anzahl der beim Quantisieren auf ±Vollaussteuerung begrenzten Samples.
    fn clipped_samples(&self) -> u64;
    /// Schließt die Datei ab (Größenfelder). Muss aufgerufen werden.
    fn finalize(self: Box<Self>) -> Result<()>;
}

/// Öffnet eine Audiodatei anhand ihres Inhalts (derzeit nur WAV).
pub fn open_source(path: &Path) -> Result<Box<dyn AudioSource>> {
    Ok(Box::new(WavReader::open(path)?))
}

/// Legt eine Audiodatei an (derzeit immer WAV, RF64 bei Bedarf).
pub fn create_sink(path: &Path, spec: AudioSpec) -> Result<Box<dyn AudioSink>> {
    Ok(Box::new(WavWriter::create(path, spec)?))
}
