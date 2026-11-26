//! Streaming-STFT mit Weighted Overlap-Add (WOLA).
//!
//! - Periodisches Hann-Fenster für Analyse **und** Synthese, Hop = N/4.
//!   Σw² = 1,5 ⇒ perfekte Rekonstruktion bei unverändertem Spektrum.
//! - Frame `k` umfasst die Eingangssamples `[(k+1)·hop − N, (k+1)·hop)`
//!   (vor dem Dateianfang: Nullen).
//! - Jedes Eingangssample erzeugt genau ein Ausgangssample; die Latenz ist N.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::f64::consts::PI;
use std::sync::Arc;

pub type Complex32 = Complex<f32>;

/// FFT-Größe für ≈ 43 ms Fensterlänge: 2048 bis 48 kHz, 4096 bis 96 kHz.
pub fn auto_fft_size(sample_rate: u32) -> usize {
    match sample_rate {
        0..=50_000 => 2048,
        50_001..=100_000 => 4096,
        _ => 8192,
    }
}

/// Periodisches Hann-Fenster.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32)
        .collect()
}

/// Ein fertiger Frame, wie ihn der Callback sieht.
pub struct StftFrame<'a> {
    /// Laufende Frame-Nummer ab 0.
    pub index: u64,
    /// Spektrum je Kanal (N/2+1 Bins). Änderungen wirken auf die Ausgabe.
    pub spectra: &'a mut [Vec<Complex32>],
    /// Unverändertes Zeitsignal des Frames je Kanal (N Samples, ungefenstert).
    pub time: &'a [Vec<f32>],
    /// Anzahl neuer Samples in diesem Frame (= hop).
    pub hop: usize,
}

pub struct StftEngine {
    n: usize,
    hop: usize,
    window: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    synthesis: bool,
    inbuf: Vec<Vec<f32>>,
    fill: usize,
    ola: Vec<Vec<f32>>,
    pending: Vec<Vec<f32>>,
    read: usize,
    spectra: Vec<Vec<Complex32>>,
    tbuf: Vec<f32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
    frame_index: u64,
    norm: f32,
}

impl StftEngine {
    /// `synthesis = false`: nur Analyse (kein Rücktransformieren, keine Ausgabe).
    pub fn new(channels: usize, n: usize, synthesis: bool) -> Self {
        assert!(
            n >= 16 && n.is_multiple_of(4),
            "FFT-Größe muss durch 4 teilbar sein"
        );
        let hop = n / 4;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_f = fwd.make_scratch_vec();
        let scratch_i = inv.make_scratch_vec();
        let window = hann(n);
        let wsum: f64 = window.iter().map(|w| (*w as f64).powi(2)).sum::<f64>() / hop as f64;
        StftEngine {
            n,
            hop,
            synthesis,
            inbuf: vec![vec![0.0; n]; channels],
            fill: 0,
            ola: vec![vec![0.0; n]; channels],
            pending: vec![vec![0.0; hop]; channels],
            read: 0,
            spectra: vec![fwd.make_output_vec(); channels],
            tbuf: vec![0.0; n],
            scratch_f,
            scratch_i,
            frame_index: 0,
            // realfft rücktransformiert unnormiert (Faktor N), dazu Σw²/hop = 1,5.
            norm: (1.0 / (n as f64 * wsum)) as f32,
            window,
            fwd,
            inv,
        }
    }

    pub fn fft_size(&self) -> usize {
        self.n
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    pub fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Verzögerung der Ausgabe gegenüber der Eingabe in Samples.
    pub fn latency(&self) -> usize {
        self.n
    }

    /// Anzahl bisher berechneter Frames.
    pub fn frames_done(&self) -> u64 {
        self.frame_index
    }

    /// Spektrum eines einzelnen Frames (je Kanal N ungefensterte Samples),
    /// unabhängig vom Streaming-Zustand. Für gezieltes Nachlesen einzelner Frames.
    pub fn spectrum_of(&mut self, time: &[Vec<f32>]) -> &[Vec<Complex32>] {
        for (c, t) in time.iter().enumerate() {
            for i in 0..self.n {
                self.tbuf[i] = t[i] * self.window[i];
            }
            self.fwd
                .process_with_scratch(&mut self.tbuf, &mut self.spectra[c], &mut self.scratch_f)
                .expect("FFT-Größen passen");
        }
        &self.spectra[..time.len()]
    }

    /// Verarbeitet `frames` Samples je Kanal. Für jeden fertigen Frame wird
    /// `on_frame` aufgerufen. Im Synthese-Modus erhält `output` genau
    /// `frames` Samples je Kanal (um [`latency`](Self::latency) verzögert);
    /// im Analyse-Modus bleibt `output` unberührt (darf leer sein).
    pub fn process<F>(
        &mut self,
        input: &[Vec<f32>],
        frames: usize,
        output: &mut [Vec<f32>],
        mut on_frame: F,
    ) where
        F: FnMut(StftFrame<'_>),
    {
        let ch = self.inbuf.len();
        if self.synthesis {
            for o in output.iter_mut() {
                o.resize(frames, 0.0);
            }
        }
        let mut pos = 0;
        while pos < frames {
            // So viele Samples wie bis zum nächsten Frame fehlen.
            let take = (self.hop - self.fill).min(frames - pos);
            let start = self.n - self.hop + self.fill;
            for c in 0..ch {
                self.inbuf[c][start..start + take].copy_from_slice(&input[c][pos..pos + take]);
                if self.synthesis {
                    output[c][pos..pos + take]
                        .copy_from_slice(&self.pending[c][self.read..self.read + take]);
                }
            }
            self.read += take;
            self.fill += take;
            pos += take;
            if self.fill == self.hop {
                self.compute_frame(&mut on_frame);
                self.fill = 0;
                self.read = 0;
            }
        }
    }

    fn compute_frame<F: FnMut(StftFrame<'_>)>(&mut self, on_frame: &mut F) {
        let ch = self.inbuf.len();
        let n = self.n;
        let hop = self.hop;
        for c in 0..ch {
            for i in 0..n {
                self.tbuf[i] = self.inbuf[c][i] * self.window[i];
            }
            self.fwd
                .process_with_scratch(&mut self.tbuf, &mut self.spectra[c], &mut self.scratch_f)
                .expect("FFT-Größen passen");
        }
        on_frame(StftFrame {
            index: self.frame_index,
            spectra: &mut self.spectra,
            time: &self.inbuf,
            hop,
        });
        self.frame_index += 1;

        if self.synthesis {
            let last = n / 2;
            for c in 0..ch {
                let spec = &mut self.spectra[c];
                // c2r verlangt reelle DC- und Nyquist-Bins.
                spec[0].im = 0.0;
                spec[last].im = 0.0;
                self.inv
                    .process_with_scratch(spec, &mut self.tbuf, &mut self.scratch_i)
                    .expect("FFT-Größen passen");
                let ola = &mut self.ola[c];
                for i in 0..n {
                    ola[i] += self.tbuf[i] * self.window[i] * self.norm;
                }
                self.pending[c].copy_from_slice(&ola[..hop]);
                ola.copy_within(hop.., 0);
                ola[n - hop..].iter_mut().for_each(|v| *v = 0.0);
            }
        }
        for c in 0..ch {
            self.inbuf[c].copy_within(hop.., 0);
        }
    }
}

/// Einfache Verzögerungsleitung (z. B. für das Delta-Signal).
pub struct DelayLine {
    buf: Vec<Vec<f32>>,
    pos: usize,
}

impl DelayLine {
    pub fn new(channels: usize, delay: usize) -> Self {
        DelayLine {
            buf: vec![vec![0.0; delay]; channels],
            pos: 0,
        }
    }

    /// Schreibt `input` und liefert das um `delay` verzögerte Signal in `output`.
    pub fn process(&mut self, input: &[Vec<f32>], frames: usize, output: &mut [Vec<f32>]) {
        let d = self.buf[0].len();
        for (c, o) in output.iter_mut().enumerate() {
            o.resize(frames, 0.0);
            if d == 0 {
                o.copy_from_slice(&input[c][..frames]);
            }
        }
        if d == 0 {
            return;
        }
        for i in 0..frames {
            for c in 0..self.buf.len() {
                output[c][i] = self.buf[c][self.pos];
                self.buf[c][self.pos] = input[c][i];
            }
            self.pos = (self.pos + 1) % d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(len: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..len)
            .map(|i| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.8
                    + 0.3 * ((i as f32) * 0.05).sin()
            })
            .collect()
    }

    #[test]
    fn identity_reconstructs_perfectly() {
        for n in [512usize, 2048, 4096] {
            let len = 50_000;
            let x = vec![signal(len, 1), signal(len, 2)];
            let mut eng = StftEngine::new(2, n, true);
            let lat = eng.latency();
            let mut padded = x.clone();
            for c in padded.iter_mut() {
                c.extend(std::iter::repeat_n(0.0, lat));
            }
            let mut out = vec![Vec::new(), Vec::new()];
            // In ungeraden Blockgrößen zuführen.
            let mut pos = 0;
            let mut y = [Vec::new(), Vec::new()];
            while pos < len + lat {
                let k = 777.min(len + lat - pos);
                let block: Vec<Vec<f32>> =
                    padded.iter().map(|c| c[pos..pos + k].to_vec()).collect();
                eng.process(&block, k, &mut out, |_| {});
                for c in 0..2 {
                    y[c].extend_from_slice(&out[c]);
                }
                pos += k;
            }
            let mut err = 0f64;
            let mut sig = 0f64;
            for c in 0..2 {
                for i in 0..len {
                    let d = (y[c][i + lat] - x[c][i]) as f64;
                    err += d * d;
                    sig += (x[c][i] as f64).powi(2);
                }
            }
            let db = 10.0 * (err / sig).log10();
            assert!(db < -120.0, "N={n}: Rekonstruktionsfehler {db:.1} dB");
        }
    }

    #[test]
    fn frame_alignment_and_count() {
        // Ein Impuls bei Sample 3000 muss im Frame k mit
        // (k+1)·hop − N <= 3000 < (k+1)·hop liegen.
        let n = 1024;
        let mut x = vec![vec![0f32; 8000]];
        x[0][3000] = 1.0;
        let mut eng = StftEngine::new(1, n, false);
        let mut hits = Vec::new();
        eng.process(&x, 8000, &mut [], |f| {
            if f.time[0].iter().any(|v| *v != 0.0) {
                hits.push(f.index);
            }
        });
        let hop = n as u64 / 4;
        for k in &hits {
            let start = (k + 1) * hop - n as u64;
            assert!(start <= 3000 && 3000 < (k + 1) * hop);
        }
        assert_eq!(hits.len(), 4);
        assert_eq!(eng.frames_done(), 8000 / hop);
    }

    #[test]
    fn gain_applies_linearly() {
        let len = 20_000;
        let x = vec![signal(len, 5)];
        let mut eng = StftEngine::new(1, 2048, true);
        let mut out = vec![Vec::new()];
        let mut padded = x.clone();
        padded[0].extend(std::iter::repeat_n(0.0, 2048));
        eng.process(&padded, len + 2048, &mut out, |f| {
            for v in f.spectra[0].iter_mut() {
                *v *= 0.5;
            }
        });
        for i in 0..len {
            assert!((out[0][i + 2048] - 0.5 * x[0][i]).abs() < 1e-5);
        }
    }

    #[test]
    fn delay_line() {
        let mut d = DelayLine::new(1, 3);
        let mut out = vec![Vec::new()];
        d.process(&[vec![1.0, 2.0, 3.0, 4.0, 5.0]], 5, &mut out);
        assert_eq!(out[0], vec![0.0, 0.0, 0.0, 1.0, 2.0]);
    }
}
