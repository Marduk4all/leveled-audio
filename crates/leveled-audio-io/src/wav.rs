//! WAV-Leser und -Schreiber mit RF64-Unterstützung (EBU Tech 3306).
//!
//! Der Schreiber legt immer einen 28-Byte-`JUNK`-Chunk an. Überschreitet die
//! Datei beim Abschließen 4 GiB, wird er in `ds64` umgewandelt und der Kopf auf
//! `RF64` umgestellt, ohne Audiodaten umzukopieren.

use crate::{AudioSink, AudioSource, AudioSpec, IoError, Result, SampleFormat};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const U32_SENTINEL: u32 = 0xFFFF_FFFF;

const SCALE16: f32 = 32768.0;
const SCALE24: f32 = 8_388_608.0;
const SCALE32: f64 = 2_147_483_648.0;

// ---------------------------------------------------------------- Leser

pub struct WavReader<R> {
    inner: R,
    spec: AudioSpec,
    data_start: u64,
    total_frames: u64,
    pos: u64,
    buf: Vec<u8>,
}

impl WavReader<BufReader<File>> {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        WavReader::new(BufReader::with_capacity(1 << 20, file))
    }
}

fn read_u16(r: &mut impl Read) -> Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn read_tag(r: &mut impl Read) -> Result<[u8; 4]> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(b)
}

struct Fmt {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bits: u16,
}

fn parse_fmt(r: &mut impl Read, size: u64) -> Result<Fmt> {
    if size < 16 {
        return Err(IoError::Malformed("fmt-Chunk zu kurz".into()));
    }
    let mut tag = read_u16(r)?;
    let channels = read_u16(r)?;
    let sample_rate = read_u32(r)?;
    let _byte_rate = read_u32(r)?;
    let block_align = read_u16(r)?;
    let bits = read_u16(r)?;
    let mut consumed = 16u64;
    if tag == WAVE_FORMAT_EXTENSIBLE {
        if size < 40 {
            return Err(IoError::Malformed("WAVE_FORMAT_EXTENSIBLE zu kurz".into()));
        }
        let _cb_size = read_u16(r)?;
        let _valid_bits = read_u16(r)?;
        let _channel_mask = read_u32(r)?;
        let mut guid = [0u8; 16];
        r.read_exact(&mut guid)?;
        // Die ersten zwei Bytes der Subformat-GUID tragen den eigentlichen Formatcode.
        tag = u16::from_le_bytes([guid[0], guid[1]]);
        consumed = 40;
    }
    skip(r, size - consumed)?;
    Ok(Fmt {
        tag,
        channels,
        sample_rate,
        block_align,
        bits,
    })
}

fn skip(r: &mut impl Read, n: u64) -> Result<()> {
    let copied = std::io::copy(&mut r.take(n), &mut std::io::sink())?;
    if copied != n {
        return Err(IoError::Malformed("Datei endet mitten im Chunk".into()));
    }
    Ok(())
}

impl<R: Read + Seek> WavReader<R> {
    pub fn new(mut inner: R) -> Result<Self> {
        let file_len = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(0))?;

        let riff = read_tag(&mut inner).map_err(|_| IoError::NotWav)?;
        let is_rf64 = match &riff {
            b"RIFF" => false,
            b"RF64" | b"BW64" => true,
            _ => return Err(IoError::NotWav),
        };
        let _riff_size = read_u32(&mut inner)?;
        if &read_tag(&mut inner)? != b"WAVE" {
            return Err(IoError::NotWav);
        }

        let mut ds64_data_size: Option<u64> = None;
        let mut fmt: Option<Fmt> = None;
        let mut pos = 12u64;
        let (data_start, data_size) = loop {
            if pos + 8 > file_len {
                return Err(IoError::Malformed("kein data-Chunk gefunden".into()));
            }
            let id = read_tag(&mut inner)?;
            let size32 = read_u32(&mut inner)?;
            pos += 8;
            match &id {
                b"ds64" => {
                    let _riff = read_u64(&mut inner)?;
                    let data = read_u64(&mut inner)?;
                    ds64_data_size = Some(data);
                    skip(&mut inner, (size32 as u64).saturating_sub(16))?;
                }
                b"fmt " => fmt = Some(parse_fmt(&mut inner, size32 as u64)?),
                b"data" => {
                    let size = if is_rf64 && size32 == U32_SENTINEL {
                        ds64_data_size
                            .ok_or_else(|| IoError::Malformed("RF64 ohne ds64-Chunk".into()))?
                    } else {
                        size32 as u64
                    };
                    break (pos, size);
                }
                _ => skip(&mut inner, size32 as u64)?,
            }
            pos += size32 as u64;
            if size32 % 2 == 1 {
                skip(&mut inner, 1)?;
                pos += 1;
            }
        };

        let fmt = fmt.ok_or_else(|| IoError::Malformed("fmt-Chunk fehlt vor data".into()))?;
        let format = match (fmt.tag, fmt.bits) {
            (WAVE_FORMAT_PCM, 16) => SampleFormat::Int16,
            (WAVE_FORMAT_PCM, 24) => SampleFormat::Int24,
            (WAVE_FORMAT_PCM, 32) => SampleFormat::Int32,
            (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleFormat::Float32,
            (WAVE_FORMAT_PCM, b) => {
                return Err(IoError::Unsupported(format!("{b}-bit PCM")));
            }
            (WAVE_FORMAT_IEEE_FLOAT, b) => {
                return Err(IoError::Unsupported(format!("{b}-bit Float")));
            }
            (t, _) => return Err(IoError::Unsupported(format!("Formatcode 0x{t:04X}"))),
        };
        if !(1..=2).contains(&fmt.channels) {
            return Err(IoError::Unsupported(format!(
                "{} Kanäle (nur Mono/Stereo)",
                fmt.channels
            )));
        }
        if !(8_000..=384_000).contains(&fmt.sample_rate) {
            return Err(IoError::Unsupported(format!(
                "Samplerate {} Hz",
                fmt.sample_rate
            )));
        }
        let spec = AudioSpec {
            sample_rate: fmt.sample_rate,
            channels: fmt.channels,
            format,
        };
        if fmt.block_align as usize != spec.block_align() {
            return Err(IoError::Malformed(format!(
                "block_align {} passt nicht zu {} Kanälen à {} Byte",
                fmt.block_align,
                fmt.channels,
                format.bytes_per_sample()
            )));
        }
        // Manche Recorder schreiben falsche Größen (z. B. nach Absturz): auf
        // die tatsächliche Dateigröße begrenzen.
        let available = file_len.saturating_sub(data_start);
        let data_size = data_size.min(available);
        let total_frames = data_size / spec.block_align() as u64;

        Ok(WavReader {
            inner,
            spec,
            data_start,
            total_frames,
            pos: 0,
            buf: Vec::new(),
        })
    }
}

impl<R: Read + Seek + Send> AudioSource for WavReader<R> {
    fn spec(&self) -> AudioSpec {
        self.spec
    }

    fn total_frames(&self) -> u64 {
        self.total_frames
    }

    fn read(&mut self, out: &mut [Vec<f32>], max_frames: usize) -> Result<usize> {
        let ch = self.spec.channels as usize;
        assert_eq!(out.len(), ch, "Kanalanzahl der Puffer passt nicht");
        let remaining = self.total_frames - self.pos;
        let frames = (max_frames as u64).min(remaining) as usize;
        for c in out.iter_mut() {
            c.clear();
        }
        if frames == 0 {
            return Ok(0);
        }
        let bps = self.spec.format.bytes_per_sample();
        self.buf.resize(frames * ch * bps, 0);
        self.inner.read_exact(&mut self.buf)?;
        decode(&self.buf, self.spec.format, out);
        self.pos += frames as u64;
        Ok(frames)
    }

    fn seek(&mut self, frame: u64) -> Result<()> {
        let frame = frame.min(self.total_frames);
        self.inner.seek(SeekFrom::Start(
            self.data_start + frame * self.spec.block_align() as u64,
        ))?;
        self.pos = frame;
        Ok(())
    }
}

fn decode(buf: &[u8], format: SampleFormat, out: &mut [Vec<f32>]) {
    let ch = out.len();
    let bps = format.bytes_per_sample();
    for (i, s) in buf.chunks_exact(bps).enumerate() {
        let v = match format {
            SampleFormat::Int16 => i16::from_le_bytes([s[0], s[1]]) as f32 / SCALE16,
            SampleFormat::Int24 => {
                let i = i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8;
                i as f32 / SCALE24
            }
            SampleFormat::Int32 => {
                (i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64 / SCALE32) as f32
            }
            SampleFormat::Float32 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        };
        out[i % ch].push(v);
    }
}

// ---------------------------------------------------------------- Schreiber

pub struct WavWriter<W: Write + Seek> {
    inner: W,
    spec: AudioSpec,
    fmt_chunk: Vec<u8>,
    data_bytes: u64,
    clipped: u64,
    allow_rf64: bool,
    buf: Vec<u8>,
}

/// Offsets im Kopf: RIFF(12) + JUNK/ds64 (8 + 28).
const JUNK_OFFSET: u64 = 12;
const JUNK_SIZE: u32 = 28;

impl WavWriter<BufWriter<File>> {
    pub fn create(path: &Path, spec: AudioSpec) -> Result<Self> {
        let file = File::create(path)?;
        WavWriter::new(BufWriter::with_capacity(1 << 20, file), spec)
    }
}

impl<W: Write + Seek> WavWriter<W> {
    pub fn new(mut inner: W, spec: AudioSpec) -> Result<Self> {
        let mut fmt = Vec::with_capacity(18);
        let tag = if spec.format.is_float() {
            WAVE_FORMAT_IEEE_FLOAT
        } else {
            WAVE_FORMAT_PCM
        };
        let block_align = spec.block_align() as u16;
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&spec.channels.to_le_bytes());
        fmt.extend_from_slice(&spec.sample_rate.to_le_bytes());
        fmt.extend_from_slice(&(spec.sample_rate * block_align as u32).to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&spec.format.bits().to_le_bytes());
        if spec.format.is_float() {
            fmt.extend_from_slice(&0u16.to_le_bytes()); // cbSize
        }

        inner.write_all(b"RIFF")?;
        inner.write_all(&0u32.to_le_bytes())?;
        inner.write_all(b"WAVE")?;
        inner.write_all(b"JUNK")?;
        inner.write_all(&JUNK_SIZE.to_le_bytes())?;
        inner.write_all(&[0u8; JUNK_SIZE as usize])?;
        inner.write_all(b"fmt ")?;
        inner.write_all(&(fmt.len() as u32).to_le_bytes())?;
        inner.write_all(&fmt)?;
        inner.write_all(b"data")?;
        inner.write_all(&0u32.to_le_bytes())?;

        Ok(WavWriter {
            inner,
            spec,
            fmt_chunk: fmt,
            data_bytes: 0,
            clipped: 0,
            allow_rf64: true,
            buf: Vec::new(),
        })
    }

    /// Bei `false` wird statt RF64 ein Fehler gemeldet, sobald 4 GiB erreicht wären.
    pub fn set_allow_rf64(&mut self, allow: bool) {
        self.allow_rf64 = allow;
    }

    fn header_len(&self) -> u64 {
        12 + 8 + JUNK_SIZE as u64 + 8 + self.fmt_chunk.len() as u64 + 8
    }

    fn finish(mut self) -> Result<W> {
        let pad = self.data_bytes % 2;
        if pad == 1 {
            self.inner.write_all(&[0])?;
        }
        let riff_size = self.header_len() - 8 + self.data_bytes + pad;
        if riff_size <= U32_SENTINEL as u64 {
            self.inner.seek(SeekFrom::Start(4))?;
            self.inner.write_all(&(riff_size as u32).to_le_bytes())?;
            self.inner.seek(SeekFrom::Start(self.header_len() - 4))?;
            self.inner
                .write_all(&(self.data_bytes as u32).to_le_bytes())?;
        } else {
            let frames = self.data_bytes / self.spec.block_align() as u64;
            self.inner.seek(SeekFrom::Start(0))?;
            self.inner.write_all(b"RF64")?;
            self.inner.write_all(&U32_SENTINEL.to_le_bytes())?;
            self.inner.seek(SeekFrom::Start(JUNK_OFFSET))?;
            self.inner.write_all(b"ds64")?;
            self.inner.write_all(&JUNK_SIZE.to_le_bytes())?;
            self.inner.write_all(&riff_size.to_le_bytes())?;
            self.inner.write_all(&self.data_bytes.to_le_bytes())?;
            self.inner.write_all(&frames.to_le_bytes())?;
            self.inner.write_all(&0u32.to_le_bytes())?; // table length
            self.inner.seek(SeekFrom::Start(self.header_len() - 4))?;
            self.inner.write_all(&U32_SENTINEL.to_le_bytes())?;
        }
        self.inner.flush()?;
        Ok(self.inner)
    }

    /// Schließt ab und gibt den inneren Writer zurück (für Tests mit `Cursor`).
    pub fn into_inner(self) -> Result<W> {
        self.finish()
    }
}

#[inline]
fn quantize(x: f32, scale: f32, min: f32, max: f32, clipped: &mut u64) -> f32 {
    let x = if x.is_nan() { 0.0 } else { x };
    let v = (x * scale).round();
    if v > max {
        *clipped += 1;
        max
    } else if v < min {
        *clipped += 1;
        min
    } else {
        v
    }
}

impl<W: Write + Seek + Send> AudioSink for WavWriter<W> {
    fn spec(&self) -> AudioSpec {
        self.spec
    }

    fn write(&mut self, channels: &[Vec<f32>], frames: usize) -> Result<()> {
        let ch = self.spec.channels as usize;
        assert_eq!(channels.len(), ch, "Kanalanzahl der Puffer passt nicht");
        let bytes = (frames * self.spec.block_align()) as u64;
        if !self.allow_rf64
            && self.header_len() - 8 + self.data_bytes + bytes + 1 > U32_SENTINEL as u64
        {
            return Err(IoError::TooLarge);
        }
        self.buf.clear();
        self.buf.reserve(bytes as usize);
        let mut clipped = 0u64;
        for i in 0..frames {
            for c in channels.iter().take(ch) {
                let x = c[i];
                match self.spec.format {
                    SampleFormat::Int16 => {
                        let v = quantize(x, SCALE16, -32768.0, 32767.0, &mut clipped);
                        self.buf.extend_from_slice(&(v as i16).to_le_bytes());
                    }
                    SampleFormat::Int24 => {
                        let v = quantize(x, SCALE24, -8_388_608.0, 8_388_607.0, &mut clipped);
                        let b = (v as i32).to_le_bytes();
                        self.buf.extend_from_slice(&b[..3]);
                    }
                    SampleFormat::Int32 => {
                        let x = if x.is_nan() { 0.0 } else { x as f64 };
                        let v = (x * SCALE32).round();
                        let v = if v > i32::MAX as f64 {
                            clipped += 1;
                            i32::MAX
                        } else if v < i32::MIN as f64 {
                            clipped += 1;
                            i32::MIN
                        } else {
                            v as i32
                        };
                        self.buf.extend_from_slice(&v.to_le_bytes());
                    }
                    SampleFormat::Float32 => {
                        let x = if x.is_finite() { x } else { 0.0 };
                        self.buf.extend_from_slice(&x.to_le_bytes());
                    }
                }
            }
        }
        self.clipped += clipped;
        self.inner.write_all(&self.buf)?;
        self.data_bytes += bytes;
        Ok(())
    }

    fn clipped_samples(&self) -> u64 {
        self.clipped
    }

    fn finalize(self: Box<Self>) -> Result<()> {
        self.finish().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn spec(format: SampleFormat, channels: u16, rate: u32) -> AudioSpec {
        AudioSpec {
            sample_rate: rate,
            channels,
            format,
        }
    }

    /// Exakt darstellbare Testwerte je Format (Vielfache des LSB).
    fn test_signal(format: SampleFormat, channels: usize, frames: usize) -> Vec<Vec<f32>> {
        let lsb = match format {
            SampleFormat::Int16 => 1.0 / 32768.0,
            SampleFormat::Int24 | SampleFormat::Int32 | SampleFormat::Float32 => 1.0 / 8_388_608.0,
        };
        (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| {
                        let phase = (i as f64 * 0.013 + c as f64).sin();
                        let steps = (phase * 0.9 / lsb).round();
                        (steps * lsb) as f32
                    })
                    .collect()
            })
            .collect()
    }

    fn roundtrip(sp: AudioSpec, data: &[Vec<f32>]) -> (Vec<Vec<f32>>, Vec<u8>) {
        let frames = data[0].len();
        let mut w = WavWriter::new(Cursor::new(Vec::new()), sp).unwrap();
        w.write(data, frames).unwrap();
        let bytes = w.into_inner().unwrap().into_inner();
        let mut r = WavReader::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(r.spec(), sp);
        assert_eq!(r.total_frames(), frames as u64);
        let mut out = vec![Vec::new(); sp.channels as usize];
        let n = r.read(&mut out, frames + 10).unwrap();
        assert_eq!(n, frames);
        (out, bytes)
    }

    #[test]
    fn roundtrip_all_formats_bitexact() {
        for format in [
            SampleFormat::Int16,
            SampleFormat::Int24,
            SampleFormat::Int32,
            SampleFormat::Float32,
        ] {
            for channels in [1u16, 2] {
                for rate in [44_100u32, 48_000, 96_000] {
                    let sp = spec(format, channels, rate);
                    let data = test_signal(format, channels as usize, 1001);
                    let (out, _) = roundtrip(sp, &data);
                    assert_eq!(out, data, "{format:?} {channels}ch {rate} Hz");
                }
            }
        }
    }

    #[test]
    fn hound_reads_our_files_and_we_read_hounds() {
        for (format, bits, float) in [
            (SampleFormat::Int16, 16, false),
            (SampleFormat::Int24, 24, false),
            (SampleFormat::Float32, 32, true),
        ] {
            let sp = spec(format, 2, 48_000);
            let data = test_signal(format, 2, 500);
            let (_, bytes) = roundtrip(sp, &data);

            // hound liest unsere Datei.
            let mut hr = hound::WavReader::new(Cursor::new(bytes)).unwrap();
            assert_eq!(hr.spec().bits_per_sample, bits);
            assert_eq!(hr.len(), 1000);
            let first: f32 = if float {
                hr.samples::<f32>().next().unwrap().unwrap()
            } else {
                hr.samples::<i32>().next().unwrap().unwrap() as f32 / (1u32 << (bits - 1)) as f32
            };
            assert_eq!(first, data[0][0]);

            // Wir lesen eine von hound geschriebene Datei.
            let hspec = hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: bits,
                sample_format: if float {
                    hound::SampleFormat::Float
                } else {
                    hound::SampleFormat::Int
                },
            };
            let mut cur = Cursor::new(Vec::new());
            {
                let mut hw = hound::WavWriter::new(&mut cur, hspec).unwrap();
                for i in 0..500 {
                    for c in 0..2 {
                        let x = data[c][i];
                        if float {
                            hw.write_sample(x).unwrap();
                        } else {
                            hw.write_sample((x * (1u32 << (bits - 1)) as f32) as i32)
                                .unwrap();
                        }
                    }
                }
                hw.finalize().unwrap();
            }
            let mut r = WavReader::new(Cursor::new(cur.into_inner())).unwrap();
            let mut out = vec![Vec::new(); 2];
            assert_eq!(r.read(&mut out, 1000).unwrap(), 500);
            assert_eq!(out, data, "{format:?}");
        }
    }

    #[test]
    fn seek_and_block_reads() {
        let sp = spec(SampleFormat::Int24, 2, 48_000);
        let data = test_signal(SampleFormat::Int24, 2, 10_000);
        let (_, bytes) = roundtrip(sp, &data);
        let mut r = WavReader::new(Cursor::new(bytes)).unwrap();
        let mut out = vec![Vec::new(); 2];
        r.seek(7_000).unwrap();
        let mut got = vec![Vec::new(); 2];
        loop {
            let n = r.read(&mut out, 999).unwrap();
            if n == 0 {
                break;
            }
            for c in 0..2 {
                got[c].extend_from_slice(&out[c]);
            }
        }
        assert_eq!(got[0], data[0][7_000..]);
        assert_eq!(got[1], data[1][7_000..]);
    }

    #[test]
    fn quantization_clips_and_counts() {
        let sp = spec(SampleFormat::Int16, 1, 48_000);
        let data = vec![vec![1.5f32, -1.5, 0.5, f32::NAN]];
        let mut w = WavWriter::new(Cursor::new(Vec::new()), sp).unwrap();
        w.write(&data, 4).unwrap();
        assert_eq!(w.clipped_samples(), 2);
        let bytes = w.into_inner().unwrap().into_inner();
        let mut r = WavReader::new(Cursor::new(bytes)).unwrap();
        let mut out = vec![Vec::new()];
        r.read(&mut out, 4).unwrap();
        assert_eq!(out[0], vec![32767.0 / 32768.0, -1.0, 0.5, 0.0]);
    }

    #[test]
    fn odd_sized_data_is_padded() {
        let sp = spec(SampleFormat::Int24, 1, 48_000);
        let data = test_signal(SampleFormat::Int24, 1, 3);
        let (out, bytes) = roundtrip(sp, &data);
        assert_eq!(out, data);
        assert_eq!(bytes.len() % 2, 0);
    }

    #[test]
    fn rf64_header_roundtrip() {
        // Ohne 4 GiB zu schreiben: den RF64-Pfad direkt prüfen, indem wir eine
        // Datei von Hand im RF64-Layout erzeugen, wie finish() es täte.
        let sp = spec(SampleFormat::Int16, 2, 48_000);
        let data = test_signal(SampleFormat::Int16, 2, 100);
        let (_, mut bytes) = roundtrip(sp, &data);
        let data_bytes = 400u64;
        let riff_size = (bytes.len() - 8) as u64;
        bytes[0..4].copy_from_slice(b"RF64");
        bytes[4..8].copy_from_slice(&U32_SENTINEL.to_le_bytes());
        bytes[12..16].copy_from_slice(b"ds64");
        bytes[20..28].copy_from_slice(&riff_size.to_le_bytes());
        bytes[28..36].copy_from_slice(&data_bytes.to_le_bytes());
        bytes[36..44].copy_from_slice(&100u64.to_le_bytes());
        let data_size_pos = bytes.len() - 400 - 4;
        bytes[data_size_pos..data_size_pos + 4].copy_from_slice(&U32_SENTINEL.to_le_bytes());
        let mut r = WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(r.total_frames(), 100);
        let mut out = vec![Vec::new(); 2];
        r.read(&mut out, 100).unwrap();
        assert_eq!(out, data);
    }

    /// Schreibt > 4 GiB in eine Datei, die nur Nullen speichert (sparse wäre
    /// plattformabhängig). Daher nur auf Anforderung: `cargo test -- --ignored`.
    #[test]
    #[ignore = "schreibt > 4 GiB auf die Platte"]
    fn rf64_written_when_exceeding_4gib() {
        let dir = std::env::temp_dir().join("leveled_audio_rf64_test.wav");
        let sp = spec(SampleFormat::Float32, 2, 96_000);
        let mut w = WavWriter::create(&dir, sp).unwrap();
        let block = vec![vec![0.25f32; 1 << 20]; 2];
        let mut frames = 0u64;
        while frames * 8 < (1u64 << 32) + (1 << 24) {
            w.write(&block, 1 << 20).unwrap();
            frames += 1 << 20;
        }
        Box::new(w).finalize().unwrap();
        let mut r = WavReader::open(&dir).unwrap();
        assert_eq!(r.total_frames(), frames);
        r.seek(frames - 10).unwrap();
        let mut out = vec![Vec::new(); 2];
        assert_eq!(r.read(&mut out, 100).unwrap(), 10);
        assert_eq!(out[1][9], 0.25);
        std::fs::remove_file(&dir).unwrap();
    }

    #[test]
    fn rejects_unsupported() {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(b"data\0\0\0\0");
        assert!(matches!(
            WavReader::new(Cursor::new(v)),
            Err(IoError::Unsupported(_))
        ));
        assert!(matches!(
            WavReader::new(Cursor::new(b"OggS0000".to_vec())),
            Err(IoError::NotWav)
        ));
    }

    #[test]
    fn extensible_format_is_read() {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&(48_000u32 * 3).to_le_bytes());
        v.extend_from_slice(&3u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&22u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        let mut guid = [0u8; 16];
        guid[0] = 1;
        v.extend_from_slice(&guid);
        v.extend_from_slice(b"LIST");
        v.extend_from_slice(&3u32.to_le_bytes());
        v.extend_from_slice(b"abc\0"); // ungerade Größe + Padding
        v.extend_from_slice(b"data");
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0]);
        let mut r = WavReader::new(Cursor::new(v)).unwrap();
        assert_eq!(r.spec().format, SampleFormat::Int24);
        let mut out = vec![Vec::new()];
        r.read(&mut out, 10).unwrap();
        assert_eq!(out[0], vec![0.5, -0.5]);
    }
}
