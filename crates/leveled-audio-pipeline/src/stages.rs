//! Bausteine aus Phase 5 (PLAN 3.11–3.13): Hochpass-Quelle, Kurven für die
//! Störgeräusch-Absenkung und die Pausen-Absenkung.
//!
//! Alle Kurven haben eine Stützstelle je STFT-Frame der Analyse (gleiche
//! Indizes wie die VAD-Klassen und die Leveler-Kurve).

use crate::settings::{GateSettings, HighpassSettings};
use leveled_audio_analysis::FrameClass;
use leveled_audio_dsp::Highpass;
use leveled_audio_io::{AudioSource, AudioSpec};

/// Einschwingzeit des Hochpasses vor einer Seek-Position (Profil-Nachlesen).
const PREROLL_S: f64 = 0.1;

/// Liest eine Quelle durch den Hochpass. Seeks starten 100 ms früher und
/// verwerfen den Vorlauf, damit der Filter eingeschwungen ist.
pub(crate) struct FilteredSource<'a> {
    inner: &'a mut dyn AudioSource,
    hp: Option<Highpass>,
    preroll: u64,
    scratch: Vec<Vec<f32>>,
}

impl<'a> FilteredSource<'a> {
    pub fn new(inner: &'a mut dyn AudioSource, s: &HighpassSettings) -> Self {
        let spec = inner.spec();
        let hp = s
            .enabled
            .then(|| Highpass::new(spec.channels as usize, spec.sample_rate, s.frequency_hz));
        FilteredSource {
            preroll: (PREROLL_S * spec.sample_rate as f64) as u64,
            scratch: vec![Vec::new(); spec.channels as usize],
            inner,
            hp,
        }
    }
}

impl AudioSource for FilteredSource<'_> {
    fn spec(&self) -> AudioSpec {
        self.inner.spec()
    }

    fn total_frames(&self) -> u64 {
        self.inner.total_frames()
    }

    fn read(&mut self, out: &mut [Vec<f32>], max_frames: usize) -> leveled_audio_io::Result<usize> {
        let n = self.inner.read(out, max_frames)?;
        if let Some(h) = self.hp.as_mut() {
            h.process(out, n);
        }
        Ok(n)
    }

    fn seek(&mut self, frame: u64) -> leveled_audio_io::Result<()> {
        let Some(h) = self.hp.as_mut() else {
            return self.inner.seek(frame);
        };
        let start = frame.saturating_sub(self.preroll);
        self.inner.seek(start)?;
        h.reset();
        let mut left = (frame - start) as usize;
        while left > 0 {
            let n = self.inner.read(&mut self.scratch, left.min(8192))?;
            if n == 0 {
                break;
            }
            h.process(&mut self.scratch, n);
            left -= n;
        }
        Ok(())
    }
}

/// Maximale zusammenhängende Bereiche `[s, e)` mit `pred(class)`.
fn runs(classes: &[FrameClass], pred: impl Fn(FrameClass) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < classes.len() {
        if !pred(classes[k]) {
            k += 1;
            continue;
        }
        let s = k;
        while k < classes.len() && pred(classes[k]) {
            k += 1;
        }
        out.push((s, k));
    }
    out
}

/// Halbe Cosinus-Rampe 0 → 1 an Position `i` von `len`.
fn rise(i: usize, len: usize) -> f32 {
    let t = (i as f64 + 0.5) / len as f64;
    (0.5 - 0.5 * (std::f64::consts::PI * t).cos()) as f32
}

/// Störgeräusch-Bereiche aus den VAD-Klassen.
pub(crate) struct KnockPlan {
    /// Gain der Tiefen (< 300 Hz) je Frame in dB (≤ 0).
    pub gain_db: Vec<f32>,
    pub regions: usize,
    pub frames: usize,
}

/// Kurve für die Störgeräusch-Absenkung: volle Absenkung in `Noise`-Frames,
/// Cosinus-Rampen von `ramp_frames` davor und danach.
pub(crate) fn knock_plan(
    classes: &[FrameClass],
    reduction_db: f64,
    ramp_frames: usize,
) -> KnockPlan {
    let n = classes.len();
    let regions = runs(classes, |c| c == FrameClass::Noise);
    let mut depth = vec![0f32; n];
    let r = ramp_frames.max(1);
    for &(s, e) in &regions {
        depth[s..e].iter_mut().for_each(|d| *d = 1.0);
        for i in 0..r {
            let w = rise(i, r);
            if let Some(k) = s.checked_sub(r - i) {
                depth[k] = depth[k].max(w);
            }
            if e + i < n {
                depth[e + i] = depth[e + i].max(1.0 - w);
            }
        }
    }
    KnockPlan {
        gain_db: depth.iter().map(|d| -(reduction_db as f32) * d).collect(),
        frames: regions.iter().map(|(s, e)| e - s).sum(),
        regions: regions.len(),
    }
}

/// Gewicht der Störgeräusch-Absenkung je FFT-Bin: voll bis 250 Hz, linear
/// auslaufend bis 350 Hz, darüber 0 (Sprachanteile bleiben).
pub(crate) fn knock_bin_weights(sample_rate: u32, fft_size: usize) -> Vec<f32> {
    (0..fft_size / 2 + 1)
        .map(|k| {
            let hz = k as f64 * sample_rate as f64 / fft_size as f64;
            ((350.0 - hz) / 100.0).clamp(0.0, 1.0) as f32
        })
        .collect()
}

/// Maske der Pausen-Absenkung je Frame (0 = Sprache, 1 = volle Absenkung).
///
/// Pausen = Bereiche ohne Sprache (auch Störgeräusch, Atmer, Stille).
/// Die VAD hält „Sprache“ `hangover_ms` über den letzten Laut hinaus; die
/// Haltezeit `hold_ms` zählt dagegen ab dem **letzten Laut**, die Absenkung
/// beginnt also schon innerhalb des VAD-Nachlaufs. Danach blendet sie über
/// `fade_out_ms` aus; vor dem nächsten Sprachbeginn ist sie `pre_open_ms`
/// früher wieder offen und blendet davor über `fade_in_ms` auf (Cosinus im
/// dB-Bereich). Pausen kürzer als `min_pause_ms` (ab dem letzten Laut)
/// bleiben unverändert. Am Dateianfang und -ende entfällt die Blende zur
/// Dateigrenze hin.
pub(crate) fn gate_mask(
    classes: &[FrameClass],
    hop_s: f64,
    g: &GateSettings,
    hangover_ms: f64,
) -> Vec<f32> {
    let n = classes.len();
    let frames = |ms: f64| (ms / 1000.0 / hop_s).round() as usize;
    let (min_f, hold_f, pre_f, out_f, in_f, hang_f) = (
        frames(g.min_pause_ms).max(1),
        frames(g.hold_ms),
        frames(g.pre_open_ms),
        frames(g.fade_out_ms).max(1),
        frames(g.fade_in_ms).max(1),
        frames(hangover_ms),
    );
    let mut mask = vec![0f32; n];
    for (s, e) in runs(classes, |c| c != FrameClass::Speech) {
        let (at_start, at_end) = (s == 0, e == n);
        // VAD-Nachlauf vor der Pause (höchstens `hang_f` Sprach-Frames).
        let tail = (0..hang_f.min(s))
            .take_while(|i| classes[s - 1 - i] == FrameClass::Speech)
            .count();
        if e - s + tail < min_f {
            continue;
        }
        let a = if at_start {
            s
        } else {
            s - tail + hold_f.min(tail + (e - s))
        };
        let b = if at_end { e } else { e.saturating_sub(pre_f) };
        if b <= a {
            continue;
        }
        let len = b - a;
        let (mut ro, mut ri) = (
            if at_start { 0 } else { out_f },
            if at_end { 0 } else { in_f },
        );
        if ro + ri > len {
            // Kurze Pause: beide Blenden anteilig kürzen.
            let scale = len as f64 / (ro + ri) as f64;
            ro = (ro as f64 * scale) as usize;
            ri = len - ro;
        }
        for (i, m) in mask[a..b].iter_mut().enumerate() {
            let down = if i < ro { rise(i, ro) } else { 1.0 };
            let up = if len - i <= ri {
                rise(len - 1 - i, ri)
            } else {
                1.0
            };
            *m = down.min(up);
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use FrameClass::{Noise, Pause, Speech};

    fn classes(spec: &[(FrameClass, usize)]) -> Vec<FrameClass> {
        spec.iter()
            .flat_map(|&(c, n)| std::iter::repeat_n(c, n))
            .collect()
    }

    #[test]
    fn knock_ramps_and_counts() {
        let c = classes(&[
            (Speech, 10),
            (Noise, 5),
            (Pause, 10),
            (Noise, 3),
            (Speech, 10),
        ]);
        let k = knock_plan(&c, 15.0, 3);
        assert_eq!(k.regions, 2);
        assert_eq!(k.frames, 8);
        assert!(k.gain_db[10..15].iter().all(|g| *g == -15.0));
        assert!(k.gain_db[..7].iter().all(|g| *g == 0.0));
        assert!(k.gain_db[7] < 0.0 && k.gain_db[7] > k.gain_db[9]);
        assert!(k.gain_db[18..21].iter().all(|g| *g == 0.0));
    }

    #[test]
    fn gate_mask_respects_hold_preopen_and_min_pause() {
        let hop_s = 0.01;
        let g = GateSettings {
            enabled: true,
            min_pause_ms: 400.0,
            hold_ms: 50.0,
            pre_open_ms: 50.0,
            fade_out_ms: 150.0,
            fade_in_ms: 50.0,
            ..Default::default()
        };
        // VAD-Nachlauf 150 ms: Die Sprach-Klasse reicht 15 Frames über den
        // letzten Laut hinaus. Letzter Laut bei Frame 85, Pause ab 100.
        // Sprache 1 s, Pause 1 s, Sprache 1 s, kurze Pause 0,15 s (+ Nachlauf),
        // Sprache, Pause bis Ende.
        let c = classes(&[
            (Speech, 100),
            (Pause, 100),
            (Speech, 100),
            (Pause, 15),
            (Speech, 50),
            (Pause, 60),
        ]);
        let m = gate_mask(&c, hop_s, &g, 150.0);
        // Absenkung beginnt 50 ms nach dem letzten Laut (Frame 90), also noch
        // im VAD-Nachlauf, und erreicht nach 150 ms die volle Tiefe.
        assert!(
            m[..90].iter().all(|v| *v == 0.0),
            "Halten nach dem letzten Laut"
        );
        assert!(m[90] > 0.0 && m[90] < 0.1);
        assert!(m[100] > 0.3 && m[100] < 0.9, "{}", m[100]);
        assert!(m[105..190].iter().all(|v| *v >= 0.99));
        assert!(
            m[195..300].iter().all(|v| *v == 0.0),
            "vor Sprachbeginn offen"
        );
        assert!(m[300..355].iter().all(|v| *v == 0.0), "kurze Pause bleibt");
        // Pause am Dateiende: kein Vorab-Öffnen, keine Blende zum Ende hin.
        assert_eq!(*m.last().unwrap(), 1.0);
        // Stetig: Ausblenden höchstens ≈ π/2/15 je Frame, Aufblenden π/2/5.
        for w in m.windows(2) {
            assert!((w[1] - w[0]).abs() <= 0.32, "{w:?}");
        }
    }
}
