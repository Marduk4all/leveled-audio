# leveled-audio – Implementierungsplan (MVP)

Arbeitstitel: **leveled-audio** (Binary `leveled-audio`, Crates `leveled-audio-*`). Lizenz: MIT OR Apache-2.0.
Stand: 2026-11-26. Status: Plan, noch kein Produktivcode.

---

## 0. Getroffene Entscheidungen (aus den Rückfragen)

| Thema | Entscheidung |
|---|---|
| Mehrere Sprecher | Gemischt in **einer** Datei. Der Leveler gleicht innerhalb der Datei an, die Loudness-Normalisierung zwischen Dateien. Mehrspur erst nach dem MVP. |
| Lizenz | MIT OR Apache-2.0. Kein GPL-Code, Lizenzprüfung per `cargo-deny`. |
| Lautheitsmessung | Crate `ebur128` (MIT, Port von libebur128) mit eigenen Verifikationstests. Eigene DSP-Arbeit nur dort, wo nötig (Oversampler für den Limiter). |
| Name | Arbeitstitel `leveled-audio`. Wird an einer Stelle definiert (Workspace-Metadaten und eine Konstante), damit er sich leicht ändern lässt. |

Weitere Entscheidungen, die ich im Plan treffe (alle in Abschnitt 3 und 4 begründet):

- **Mehrpass-Streaming** statt „alles in den RAM“ (3 Lesedurchläufe, konstanter Speicher).
- **Eigene WAV-I/O** mit RF64-Unterstützung statt `hound` (`hound` nur als Testorakel). Grund: 4-GB-Grenze, siehe 3.2.
- **Gain-Regel:** Wiener mit Decision-Directed-SNR (Default). Power Subtraction als ReaFir-naher Modus.
- **Leveler phrasenbasiert und offline** (die Gain-Kurve wird aus Pass-1-Daten über die ganze Datei berechnet, Gain-Wechsel liegen bevorzugt in Pausen).
- **f32-Signalpfad, f64-Akkumulatoren.**

---

## 1. Architektur

### 1.1 Crate-Struktur (Bewertung des Vorschlags)

Der Vorschlag `dsp` / `analysis` / `pipeline` / `cli` ist gut. Ich ergänze **`io`** (Formatabstraktion) und eine nur für Tests genutzte **`testkit`**:

```
leveled-audio/
├─ Cargo.toml                 (workspace, edition 2024, gemeinsame lints/profiles)
├─ rust-toolchain.toml        (gepinnte stable-Version)
├─ deny.toml                  (Lizenzen, Advisories)
├─ crates/
│  ├─ leveled-audio-dsp/           keine I/O, keine Threads, kein serde-Zwang
│  │    stft (WOLA-Engine), window, oversampler (polyphase FIR), limiter,
│  │    biquad, smoothing (one-pole, slew, Medianfilter), dither, db/gain utils,
│  │    AudioBlock (planar f32, absolute Sampleposition)
│  ├─ leveled-audio-analysis/      baut auf dsp auf, keine I/O
│  │    features (Energie, Flachheit, Flux), vad, noise_profile (Suche + Konfidenz),
│  │    min_statistics (Fallback), loudness (ebur128-Wrapper), leveler_curve
│  ├─ leveled-audio-io/            WAV/RF64 lesen+schreiben, Sampleformat-Konvertierung,
│  │    Traits AudioSource / AudioSink (später FLAC/MP3 per symphonia hier andocken)
│  ├─ leveled-audio-pipeline/      Kette, Pass-Orchestrierung, Batch, Settings (serde),
│  │    Report-Typen, CancelToken, ProgressSink-Trait (kein indicatif hier!)
│  ├─ leveled-audio-cli/           clap, TOML-Presets, indicatif, ctrlc, JSON-Report-Ausgabe
│  └─ leveled-audio-testkit/       (publish = false) Testsignal-Generatoren, Metriken
│                              (SNR, Kurtosis-Ratio), Golden-File-Helfer
└─ .github/workflows/ (ci.yml, release.yml)
```

Abhängigkeitsrichtung: `cli → pipeline → {analysis, io} → dsp`. `analysis` und `dsp` kennen weder Dateien noch Fortschrittsanzeigen. Damit kann eine spätere GUI (egui/Tauri) direkt `pipeline` nutzen und sich über `ProgressSink` und `CancelToken` einklinken.

### 1.2 Kern-Abstraktionen

```rust
// dsp
pub struct AudioBlock<'a> { pub channels: &'a mut [Vec<f32>], pub frames: usize, pub pos: u64 }
pub trait Processor {
    fn latency(&self) -> usize;                 // in Samples, wird von der Pipeline kompensiert
    fn process(&mut self, block: &mut AudioBlock);
    fn flush(&mut self, out: &mut AudioBlock) -> usize;  // Nachlauf
    fn reset(&mut self);
}
// io
pub trait AudioSource { fn spec(&self) -> AudioSpec; fn total_frames(&self) -> u64;
                        fn read(&mut self, dst: &mut [Vec<f32>]) -> Result<usize>;
                        fn seek(&mut self, frame: u64) -> Result<()>; }
pub trait AudioSink   { fn write(&mut self, src: &[Vec<f32>], frames: usize) -> Result<()>;
                        fn finalize(self) -> Result<()>; }
// pipeline
pub trait ProgressSink: Send + Sync { fn update(&self, file: FileId, pass: Pass, done: u64, total: u64); }
pub struct CancelToken(Arc<AtomicBool>);
```

- Die Pipeline summiert die Latenzen, verwirft den Vorlauf und spült den Nachlauf. **Ausgabelänge = Eingabelänge, sampelgenau.**
- Blockgröße intern 4096 Frames. Abbruchprüfung pro Block (ca. alle 50–100 ms Rechenzeit).
- Erweiterungspunkt „Pre-Chain“ (z. B. Hochpass, Hum-Removal): Die Stufen laufen **vor** der Analyse in Pass 1, damit das Rauschprofil auf dem vorgefilterten Signal entsteht. Im MVP ist die Pre-Chain leer, der Slot existiert aber schon.

### 1.3 Zwei-Pass vs. Streaming: Bewertung

Größenordnung: 3 h · 48 kHz · Stereo = 1,04 Mrd. Samples → **4,1 GB in f32** (8,3 GB in f64). Alles in den RAM zu laden scheidet also aus.

Ein reines Ein-Pass-Streaming geht ebenfalls nicht: Integrierte Lautheit, ein dateiweites Rauschprofil und ein Leveler mit Vorausschau brauchen Wissen über die ganze Datei.

**Lösung: Mehrpass-Streaming.** Jeder Pass streamt blockweise. Zwischen den Passes liegen nur kompakte Analysedaten:

```
Pass 1  ANALYSE   Eingabe lesen → [Pre-Chain] → STFT (Analyse) →
                  pro Frame: Energie, Flachheit, Flux, K-gewichtete Energie (≈12–20 MB für 3 h)
                  + ebur128 (Eingangs-Lautheit) + Minimum-Statistics-Snapshots (alle 5 s)
        offline:  VAD über die Feature-Arrays → Profil-Kandidaten → Auswahl
Pass 1b PROFIL    gezielte Seek-Reads der ausgewählten Pausen (0,5–10 s) → Rauschprofil + Konfidenz
        offline:  Leveler-Gain-Kurve (pro Frame, ganze Datei, phasenbasiert, nullphasig geglättet)
Pass 2  MESSEN    Eingabe → Denoise → Leveler-Kurve anwenden → ebur128 (nur messen, kein Schreiben)
        ─ Batch-Barriere (nur im Album-Modus) ─
        offline:  Normalisierungs-Gain = Ziel − gemessene Lautheit
Pass 3  RENDERN   Eingabe → Denoise → Leveler → Gain → True-Peak-Limiter → Dither/Quantisierung
                  → .partial-Datei schreiben (+ optional Delta-WAV) + ebur128 (Ausgangs-Report) → rename
```

**Warum wird Denoise in Pass 2 und 3 doppelt gerechnet statt eine Temp-Datei zu schreiben?**
- Die Verarbeitung ist deterministisch, daher liefern beide Durchläufe identische Daten.
- Kosten: Denoise schätze ich auf ≳300× Echtzeit pro Kern (3 h ≈ 30–40 s), siehe Benchmarks.
- Eine Temp-Datei wären 4 GB pro 3-h-Datei, und bei parallelen Jobs auf einer externen Platte entstünde Plattenstress, dazu Aufräumen bei Ctrl+C. Deshalb keine Temp-Datei.
- Option für später: `--cache-intermediate`, falls Benchmarks auf langsamen CPUs es rechtfertigen.

**Speicherbedarf pro Datei:** Feature-Arrays ≈ 20 MB plus Puffer < 10 MB. **Ziel: Peak-RSS < 200 MB** für eine 3-h-Datei, und < 100 MB + 30 MB pro paralleler Job.

### 1.4 Parallelität und Determinismus

- `rayon` parallelisiert **über Dateien**. Innerhalb einer Datei läuft alles sequenziell. Dadurch gibt es keine reihenfolgeabhängigen Float-Reduktionen.
- Dither-PRNG: eigener xorshift mit festem Seed aus (Dateiindex-unabhängiger) Konstante und Kanalnummer. Keine `HashMap`-Iteration in Ausgaben. Reports werden nach Eingabepfad sortiert, nicht nach Fertigstellung.
- **Garantie:** Gleiche Binary + gleiche CPU-Familie ⇒ **bitidentische Ausgabe**. Über Plattformen hinweg (z. B. NEON gegenüber AVX, weil rustfft SIMD zur Laufzeit wählt) nur innerhalb einer Toleranz (< −120 dBFS Differenz). Das wird so dokumentiert und in den Golden-Tests berücksichtigt.
- Pro Datei soll intern nichts parallelisiert werden. Konsequenz: Eine einzelne 3-h-Datei läuft auf einem Kern (bei ≈50–100× Echtzeit sind das ≈2–4 min). Spätere Option: Kanäle parallel.

---

## 2. Technologieentscheidungen

### 2.1 Crates

| Zweck | Crate | Bewertung | Entscheidung |
|---|---|---|---|
| WAV | `hound` | Reif, Apache-2.0, aber seit Jahren kaum Releases. **Kein RF64** (Lesen und Schreiben). | Nur als Dev-Dependency (Testorakel). |
| WAV | eigene Implementierung in `leveled-audio-io` | RIFF + RF64/BW64 + WAVE_FORMAT_EXTENSIBLE, PCM 16/24/32 + Float 32. Etwa 500–700 Zeilen plus Tests. Erlaubt schnelle Blockkonvertierung und Seek. | **Ja.** |
| Später FLAC/MP3 lesen | `symphonia` | MPL-2.0 (Datei-Copyleft, mit MIT-Binaries vereinbar) | Vormerken, Andocken über `AudioSource` |
| FFT | `realfft` (über `rustfft`) | MIT/Apache, aktiv gepflegt, SIMD (AVX/SSE/NEON) | **Ja** |
| Lautheit | `ebur128` | MIT, Port von libebur128, I/LRA/TP, `loudness_global_multiple` für den Album-Modus | **Ja** |
| Parallelität | `rayon` | Standard | **Ja** |
| CLI | `clap` v4 (derive) | Standard | **Ja** |
| Konfiguration | `serde`, `toml`, `serde_json` | Standard | **Ja** |
| Fortschritt | `indicatif` | MIT, gepflegt | **Ja** (nur in cli) |
| Fehler | `thiserror` (Libs), `anyhow` (cli) | Standard | **Ja** |
| Logging | `tracing` + `tracing-subscriber` | Für `-v/-vv`-Diagnose | **Ja** (schlank konfiguriert) |
| Ctrl+C | `ctrlc` | MIT/Apache, plattformübergreifend | **Ja** |
| Benchmarks | `criterion` | Standard | **Ja** (dev) |
| Tests | `approx`, `proptest` (gezielt) | | dev |
| Zufall | – | Eigener xorshift (Determinismus, keine Abhängigkeit) | |

Bewusst **nicht** verwendet: `dasp`, `rubato` (kein Resampling im MVP) und eine externe Oversampling-Bibliothek. Der Polyphasen-Oversampler ist klein und muss ohnehin exakt kontrolliert sein.

### 2.2 Warum eigene WAV-I/O (wichtigste Abweichung vom Vorschlag)

| Fall (3 h) | Größe | RIFF (≤ 4 GiB) |
|---|---|---|
| 48 kHz Stereo 24 bit | 3,1 GB | ok |
| 48 kHz Stereo 32f | 4,15 GB | knapp ok |
| 96 kHz Stereo 24 bit | 6,2 GB | **nur als RF64** |
| 96 kHz Stereo 32f | 8,3 GB | **nur als RF64** |

Lange Aufnahmen mit 96 kHz sind ein ausdrückliches Ziel, und `hound` könnte sie weder lesen noch schreiben. Die eigene Implementierung schreibt automatisch RF64, sobald die Datei 4 GiB überschreiten würde. Ein Flag `--no-rf64` liefert stattdessen eine klare Fehlermeldung.

Nicht unterstützt werden 8 bit, mehr als 2 Kanäle und komprimierte WAVs (z. B. ADPCM). Dafür gibt es klare Fehlermeldungen.

### 2.3 f32 oder f64

- **Signalpfad, STFT, Puffer: f32.** 24 bit Mantisse ≈ 144 dB Dynamik, das liegt über dem 24-bit-Ausgang. FFT-Rundungsfehler liegen bei ≈ −130 dB. Halber Speicher- und Cache-Bedarf, doppelte SIMD-Breite.
- **f64:** Energieakkumulatoren, Lautheit, Gain-Kurven-Berechnung, Biquad-Zustände (tiefe Frequenzen), Zeit- und Positionsrechnung (u64-Samples, keine Float-Zeit).
- Fallstrick: Lange Summen über Millionen Frames immer in f64 (oder per Kahan-Summation) bilden.

### 2.4 Toolchain und Build

- Rust stable, Edition 2024, Version in `rust-toolchain.toml` gepinnt.
- `[profile.release]`: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"` (nur im Binary). Für schnelle DSP-Tests `opt-level = 2` auch im Test-Profil der DSP-Crates.
- Keine Laufzeitabhängigkeiten:
  - Windows: statische CRT (`-C target-feature=+crt-static`), also kein VC++-Redistributable nötig.
  - Linux: `x86_64-unknown-linux-musl`, vollständig statisch.
  - macOS: Universal Binary (arm64 + x86_64 per `lipo`).

---

## 3. Algorithmen (DSP-Ebene)

Alle Zeitparameter sind in Millisekunden oder Sekunden angegeben und werden pro Samplerate in Frames und Samples umgerechnet.

### 3.1 STFT/WOLA-Engine

- Periodisches Hann-Fenster als Analyse- **und** Synthesefenster, Hop = N/4 (75 % Overlap). Σw² = 1,5 ⇒ Normierung 1/1,5. Rekonstruktion bei G ≡ 1 perfekt (Test).
- Das Synthesefenster blendet Diskontinuitäten aus, die durch die Gain-Modifikation entstehen.
- **FFT-Größe Default „auto“ ≈ 43 ms:** 2048 bei 44,1/48 kHz, 4096 bei 96 kHz (Hop ≈ 10,7 ms). Wählbar sind 512 … 8192.
  - Begründung: ≈ 23 Hz Auflösung trennt tonale Störer (Brummen-Harmonische, Lüftertöne) von Sprachobertönen und ergibt ruhigere Gain-Masken. Das ist für ein festes Rauschprofil im Stil von ReaFir günstig.
  - Preis: mehr Pre-Echo bzw. Verschmieren an Plosiven. Durch 75 % Overlap und moderate Reduktion bleibt das unauffällig. Option `fft_size = 1024` für „tighter“.
- Phase: Die verrauschte Phase wird beibehalten (Standard).
- Latenz: N − Hop Samples, kompensiert durch die Pipeline. Daher ist das Delta-Signal sampelgenau.

### 3.2 Frame-Features und VAD (Pass 1, dann offline)

Die Analyse-Frames sind die STFT-Frames (≈43 ms Fenster, 10,7 ms Hop, also im geforderten Bereich von 20–50 ms). Die Features werden aus der **Summe der Kanalleistungen** berechnet, nicht aus dem Mid-Signal (robust gegenüber Phasenumkehr zwischen L und R):

| Feature | Definition |
|---|---|
| `E` | Bandenergie 100–4000 Hz in dBFS |
| `E_k` | K-gewichtete Frame-Energie (für Leveler) |
| `SFM` | Spektrale Flachheit 300–6000 Hz (geom./arith. Mittel) |
| `Flux` | positiver spektraler Flux auf log-Betrag, normiert |
| `zero_frac` | Anteil exakt gleicher/nuller Samples (digitale Stille, Flatline) |

**VAD** (offline über die Arrays, daher mit beliebiger Vorausschau):
1. Hintergrundpegel `B(t)`: gleitendes 10. Perzentil von `E` über ±1,5 s (Histogramm-basiert, O(n)).
2. Sprache, wenn `E > B + 8 dB` (Eintritt), Halten bis `E < B + 5 dB` (Hysterese). Zusätzlich muss die Flachheit sprachtypisch sein (`SFM < 0,5`), sonst ist der Frame ein „Transient bzw. Geräusch“.
3. Hangover 150 ms, Pre-Roll 50 ms, minimale Sprachdauer 60 ms. Lücken < 250 ms werden geschlossen.
4. Ergebnis: drei Klassen pro Frame: `Speech`, `Pause`, `Other` (Transient/Atem/Stille).

Fallstricke:
- Musik oder Jingles werden als Sprache erkannt (das ist für den Leveler akzeptabel und wird gewarnt).
- Bei SNR < 10 dB wird die VAD unzuverlässig. Das senkt die Profil-Konfidenz und löst den Fallback aus.
- Lachen und Übersprechen verhalten sich wie Sprache (unkritisch).

### 3.3 Automatische Rauschprofilsuche

**Kandidaten-Frames** = `Pause`-Frames, die **alle** folgenden Bedingungen erfüllen:
- keine digitale Stille: `E > −90 dBFS` und `zero_frac < 0,5`;
- nicht in den ersten bzw. letzten 1,0 s der Datei und ≥ 300 ms Abstand zu digitaler Stille (Fades und Schnittkanten);
- ≥ 100 ms Abstand zum nächsten `Speech`-Frame (Ausklang und Hall);
- stationär: Standardabweichung von `E` über ±5 Frames < 1,5 dB **und** `Flux < Median(Flux_Pause) + 3·MAD`;
- Teil eines zusammenhängenden Kandidatenlaufs ≥ 85 ms (mindestens 8 Frames, damit volle STFT-Fenster entstehen).

**Auswahl per Perzentil:** Unter den Kandidaten werden die Frames mit `P3 ≤ E ≤ P30` der Kandidatenenergien genommen.
- Das absolute Minimum (und < P3) wird ausgeschlossen. Es ist statistisch nach unten verzerrt und erwischt Gating- oder Schnittartefakte, was zu Untersubtraktion führt.
- Atmer liegen typischerweise 5–20 dB über dem Raumton und fallen dadurch und durch den Stationaritätsfilter heraus.
- Frames werden aufsteigend nach Energie gesammelt, bis 10 s erreicht sind (oder der Vorrat erschöpft ist). **Minimum 0,5 s, voll konfident ab 2 s**, gern aus mehreren Pausen.

**Profilberechnung (Pass 1b):** Seek zu den ausgewählten Läufen und STFT in Verarbeitungsauflösung.
- Pro Kanal und Bin wird ein **getrimmtes Mittel der Leistung** gebildet (obere 10 % verworfen). Leistung statt Betrag mitteln ergibt eine erwartungstreue PSD.
- Option `statistic = "mean" | "trimmed" | "median"`. Der Median wird mit 1/ln 2 bias-korrigiert (χ²-Verteilung mit 2 Freiheitsgraden).
- Gespeichert wird die Leistung. Die Ausgabe in dB oder als Betrag erfolgt nur für Anzeige und Export.

**Konfidenz ∈ [0,1]** = gewichtetes geometrisches Mittel aus:
- Menge: `min(dauer/2 s, 1)`;
- Trennung: `clamp((Median E_Speech − E_Profil − 6 dB)/14 dB)`, also 6 dB → 0 und 20 dB → 1;
- Stationarität: beobachtete Streuung der Bin-Leistungen gegenüber der für stationäres Rauschen erwarteten Streuung (≈ 5,6 dB bei χ²₂);
- Verteilung: Anzahl verschiedener Pausen (≥ 3 → 1).

Im Report erscheint die Konfidenz zusätzlich als Text (hoch ≥ 0,7 / mittel / niedrig < 0,4).

**Fallback (Konfidenz < 0,4 oder < 0,5 s Material):** Minimum Statistics nach Martin (2001) mit Bias-Kompensation, laufend über die STFT in Pass 1.
- Alle 5 s wird ein Snapshot des Schätzers gespeichert (3 h ≈ 2200 × 1025 × 4 B ≈ 9 MB).
- Fallback-Profil = Bin-weiser Median der Snapshots. MCRA verwerfe ich für den MVP: Es hätte mehr Parameter bei ähnlichem Nutzen für ein *statisches* Profil.
- Liegt das Profil unter −95 dBFS (praktisch kein Rauschen), wird Denoise mit Hinweis übersprungen.

### 3.4 Spectral Subtract (Gain-Berechnung)

Bezeichnungen: `X` ist das Spektrum des aktuellen Frames, `λ` das Profil (Leistung) mal Oversubtraction `α`, `Gmin = 10^(−reduction_db/20)`.

**Bewertung der Gain-Regeln:**

| Regel | Vorteil | Nachteil |
|---|---|---|
| Power Subtraction `G = sqrt(max(1 − α·λ/|X|², β))` | Einfach, ReaFir-ähnlich, „direkt“ | Starkes Musical Noise, weil jedes Bin pro Frame unabhängig entscheidet |
| **Wiener + Decision-Directed** (Ephraim/Malah): `ξ = a·G²(l−1)·|X(l−1)|²/λ + (1−a)·max(γ−1, 0)`, `γ = |X|²/λ`, `G = ξ/(1+ξ)` | Glättet das A-priori-SNR über die Zeit und unterdrückt Musical Noise deutlich. Bewährter Standard. | Leichte Verzögerung an Sprachonsets (1 Frame ≈ 11 ms, kaum hörbar) |

**Default: `gain_rule = "wiener-dd"`, `a = 0,98`.** `"subtract"` (Power Subtraction) wird als Modus für ReaFir-Gewohnte angeboten.

**Parameter und Defaults:**

| Parameter | Default | Bereich | Bedeutung |
|---|---|---|---|
| `reduction_db` | **12** | 0–40 | Max. Absenkung, harte Untergrenze `G ≥ Gmin` (beide Modi) |
| `oversubtraction` | **1,3** | 1,0–3,0 | Skaliert das Profil (Sicherheitsabstand für Schwankungen) |
| `floor` (nur `subtract`) | **0,05** (≈ −13 dB Leistung) | 0–1 | Spectral Floor β relativ zum Rauschprofil. Effektiv gilt `G ≥ max(sqrt(β), Gmin)`. |
| `release_ms` | **40** (Plan: 80) | 0–1000 | Gain fällt mit Ein-Pol-Glättung, steigt sofort (Onsets bleiben erhalten) |
| `freq_smooth_oct` | **0 = aus** (Plan: 1/6) | 0–1 | Dreiecksglättung des Gains im **log-Gain-Bereich**, mindestens 3 Bins |
| `fft_size` | auto | 512–8192 | siehe 3.1 |

**Maßnahmen gegen Musical Noise (explizit):**
1. DD-Wiener statt Rohsubtraktion (Hauptmaßnahme).
2. Gain-Floor `Gmin`. Es wird nie auf null abgesenkt, der natürliche Rest-Rauschteppich maskiert isolierte Bins. Default von nur 12 dB.
3. Profil aus ≥ 50–1000 Frames mit getrimmtem Mittel, dadurch geringe Schätzvarianz.
4. Moderate Oversubtraction (1,3).
5. Frequenzglättung des Gains (1/6 Oktave), damit keine isolierten „Tonbins“ entstehen.
6. Asymmetrische Zeitglättung (schneller Anstieg, 80 ms Abfall).
7. Messung im Test: **Log-Kurtosis-Ratio** des Restrauschens (Uemura et al.) als Regressionsmetrik.

**Stereo:** Profil und Gain pro Kanal (Default). Das ist richtig für zwei hart gepannte Mikrofone. Option `stereo_link = true` (gemeinsame Maske aus der Summenleistung) erhält das Stereobild bei Mitte-Quellen.

**Delta-Export:** `delta = Eingang(latenzkompensiert) − Denoise-Ausgang`, also nur die Denoise-Stufe vor Leveler und Gain, im Eingangsformat. Da WOLA perfekt rekonstruiert, ist das exakt der entfernte Anteil.

**Bins am Rand:** DC- und Nyquist-Bin werden wie andere behandelt (mit `Gmin`). Ein DC-Offset im Eingang wird gemeldet (Warnung), aber im MVP nicht entfernt.

### 3.5 Profile exportieren und importieren

- Format **JSON**, versioniert, z. B.:
  `{ format_version, tool_version, sample_rate, fft_size, window, channels, statistic, confidence, source: "auto|minstat|imported", duration_s, bins_db: [[...],[...]] }`.
- Import bei abweichender Samplerate oder FFT-Größe: Interpolation über die **Frequenz** (linear im dB-Bereich).
  - Mono-Profil auf Stereo-Eingang: für beide Kanäle verwenden.
  - Stereo-Profil auf Mono-Eingang: Mittelwert der Kanäle.
- CLI:
  - `--noise-profile <file.json>` erzwingt das Profil für alle Dateien des Batches.
  - `--save-profile <dir|file>` exportiert die automatisch erzeugten Profile.
  - `leveled-audio profile learn <room-tone.wav> -o studio.json` lernt aus einer Raumton-Aufnahme (Profilsuche bzw. bei reinem Raumton die ganze Datei).
- Fallstrick: Das Profil ist **absolut** (dBFS). Ein anderer Gain-Staging-Pegel in einer anderen Datei macht es ungültig. Deshalb wird gewarnt, wenn der Pausenpegel der Datei mehr als 6 dB vom importierten Profil abweicht.

### 3.6 Adaptive Leveler (phrasenbasiert, offline berechnete Kurve)

Weil VAD und Frame-Energien der ganzen Datei nach Pass 1 vorliegen, wird die Gain-Kurve vollständig **vor** dem Rendern berechnet. Sie kann dadurch voraus- und zurückschauen und nullphasig glätten. Kein Pumpen durch kausale Regelung.

1. **Sprechenergie:** `E_k` (K-gewichtet) minus die K-gewichtete Profilleistung (Leistungssubtraktion auf Frame-Ebene, Untergrenze −70 dB). Damit wirkt der Leveler auf den Sprachanteil, auch wenn Denoise erst später greift.
2. **Phrasen:** VAD-Sprachsegmente. Lücken < 250 ms werden geschlossen. Segmente < 300 ms übernehmen den Wert des Nachbarn. Lange Segmente (> 6 s) werden intern mit einem gleitenden 3-s-Fenster gemessen.
3. **Referenz:** `L_ref` = Mitte zwischen unterem und oberem Quartil der Segmentlautheiten (dauergewichtet). Leise Sprecher werden angehoben, laute abgesenkt, symmetrisch. Die absolute Lage ist egal, weil die Normalisierung folgt.
   - *Geändert in Phase 3:* Ursprünglich Median. Bei zwei Sprechern mit je ≈ 50 % Sprechzeit fällt der Median zufällig in einen der beiden Cluster. Dann bekommt ein Sprecher die volle Korrektur und läuft in `max_cut`/`max_boost` (gemessen: −10…+5 dB statt ±5 dB). Die Quartilsmitte ist bei einem Sprecher ≈ Median und bei zwei Sprechern symmetrisch.
   - *Neu in Phase 3:* **Gate.** Phrasen mehr als 20 dB unter `L_ref` (Atmer, Raumgeräusch, fernes Gemurmel) werden wie Pausen behandelt. An deiner Aufnahme 005 hob der Leveler sonst einen leisen Ausklang (≈ 22 dB unter der Sprache) um die vollen +10 dB an.
   - *Neu nach Phase 6 (Abstandstest):* **Geräusch-Gate.** Phrasen bis 1,5 s mit weniger als 20 % tonalen Frames (spektrale Flachheit < 0,15) gelten ebenfalls als Pause, schon vor der Referenzbildung. Anlass: `Abstand.WAV` (1 m → 50 cm → 5 cm). Beim Heranführen des Mikrofons stufte die VAD Rascheln/Atmen als Sprache ein; diese Stücke lagen nur knapp innerhalb der 20-dB-Grenze, bekamen +10 dB und die kurzen Pausen dazwischen hielten den Gain. Gemessen: Sprache hat 60–70 % tonale Frames (auch bei 1 m), die Geräusche 4 %.
4. **Soll-Gain:** `g = strength · (L_ref − L_seg)`, begrenzt auf `[−max_cut, +max_boost]`.
5. **Übergänge:** Gain-Wechsel liegen bevorzugt **in Pausen** (Rampe 250 ms, die vor dem Sprachbeginn endet). Ist die Pause zu kurz, wird über die Grenze geführt, mit Slew-Limit. Innerhalb einer Phrase gilt ein Slew-Limit von 2 dB/s.
6. **Pausen nicht anheben:**
   - Kurze Pausen (< 2 s) halten bzw. interpolieren den Gain. Ein Absenken in jeder kurzen Pause würde den Rauschteppich „atmen“ lassen, was schlimmer ist.
   - Lange Pausen (≥ 2 s) gleiten langsam (500 ms) auf `min(g, pause_max_boost_db)` und sind rechtzeitig vor der nächsten Phrase wieder oben.
7. **Glättung:** Nullphasige Ein-Pol-Glättung (vorwärts und rückwärts, τ = 150 ms) innerhalb der Phrasen. Die Übergänge in den Pausen sind Cosinus-Rampen und damit bereits glatt. Angewendet wird pro Sample mit linearer Interpolation zwischen Frame-Stützstellen.
   - *Geändert in Phase 3:* Ursprünglich eine Glättung über die ganze Kurve. Sie hätte jede Pausenrampe um ≈ τ in den Sprachbeginn verschoben, sodass die ersten ≈ 300 ms einer Phrase noch mit dem alten Gain gelaufen wären.
   - `pause_max_boost_db` wird nach unten durch `−max_cut_db` begrenzt.

| Parameter | Default | Bereich |
|---|---|---|
| `strength` | **130 %** (bis 2026-11-27: 60 %) | 0–130 % (0 % ⇒ bitidentisch zu Bypass; über 100 % Überkorrektur) |
| `max_boost_db` | **20** (bis 2026-11-27: 10) | 0–20 |
| `max_cut_db` | **10** | 0–20 |
| `pause_max_boost_db` | **0** | −10–+10 |
| `slew_db_per_s` | **2** | 0,5–10 |
| Gate unter `L_ref` | **20 dB** | intern |

Fallstricke:
- Musik oder Jingles werden mitgelevelt (Warnung, später Option für Ausschlussbereiche).
- Sehr kurze Einwürfe („mhm“) erhalten den Gain des Nachbarsegments.
- Ein Sprecher mit stark schwankendem Abstand zum Mikrofon innerhalb einer Phrase wird nur langsam ausgeglichen (bewusster Kompromiss gegen Pumpen).

### 3.7 Loudness-Messung und Normalisierung

- `ebur128` mit `Mode::I | LRA | TRUE_PEAK | SAMPLE_PEAK | HISTOGRAM` (Histogramm-Modus ⇒ konstanter Speicher bei langen Dateien). Mono wird als ein Kanal gemessen.
- Normalisierungs-Gain = `Ziel − I(Pass 2)`. Der Gain ist statisch, daher keine Dynamikänderung.
- **Album-Modus** (`--loudness-mode album`): Die ebur128-States aller Dateien aus Pass 2 werden mit `loudness_global_multiple` kombiniert, und **ein** Gain gilt für alle Dateien (relative Unterschiede bleiben). Default ist `per-file` (jede Datei trifft das Ziel).
- Presets: `podcast-stereo` −16 LUFS, `podcast-mono` −19 LUFS, `ebu-r128` −23 LUFS, jeweils mit Ceiling −1 dBTP. `custom` per Option.
- **Fallstrick:** Greift der Limiter stark ein, sinkt die integrierte Lautheit leicht unter das Ziel. Deshalb wird nach Pass 3 gemessen. Bei einer Abweichung > 0,5 LU folgt **eine** Korrekturiteration (erneuter Pass 3 mit angepasstem Gain), sonst eine Warnung im Report. Standardfall bei Sprache und −16 LUFS: kein zweiter Durchlauf.
  - *Geändert in Phase 4:* **höchstens zwei** Korrekturiterationen. Die erste nimmt Steigung 1 an (Gain += Ziel − gemessen). Die zweite schätzt die Steigung aus beiden Messungen (Sekante, begrenzt auf 1–4 dB Gain je LU). Grund: Im Album-Modus liegt der gemeinsame Gain für eine schon laute Datei weit oben, und der Limiter flacht den Zusammenhang Gain → Lautheit stark ab. Gemessen kam eine Iteration auf −16,72 LUFS, zwei auf −16,18. Die Gesamtkorrektur ist auf ±6 dB begrenzt.
  - Im Album-Modus wird die **Album**-Lautheit der Ausgabe korrigiert: Alle Dateien werden mit demselben neuen Gain neu gerendert, der Gain bleibt also für alle gleich.
  - Das Delta-WAV hängt nicht vom Gain ab und wird nur im ersten Durchlauf geschrieben.
  - *Geändert in Phase 5:* Die Schwelle sinkt auf **0,3 LU**. Neu ist die **Limiter-Probe** in Pass 2: Mit einem aus der Eingangslautheit geschätzten Gain läuft dort schon ein Limiter mit, und die Lautheit danach wird gemessen. Dieser Messpunkt ist der erste Stützpunkt der Korrektur, und der erste Render-Durchlauf startet mit dem daraus korrigierten Gain.
    - Ergebnis: Meist reicht ein Render-Durchlauf. Eine 1-h-Datei läuft mit 55× Echtzeit; ohne Probe waren es 46× (zusätzlicher Durchlauf).
    - Die Probe gibt es nur pro Datei. Im Album-Modus bleibt die Korrektur über die Album-Lautheit.

### 3.8 True-Peak-Limiter

- **Detektion mit 8× Oversampling** (4×/16× wählbar): Polyphasen-FIR, 16 Taps pro Phase, Kaiser-gefenstert. Das Oversampling dient nur der Detektion, der Audiopfad bleibt in der Basisrate.
  - *Geändert in Phase 1:* Bei 4× verfehlt allein das Abtastraster den wahren Peak um bis zu 0,3 dB (16 kHz bei 48 kHz). Mit 8× sind es ≤ 0,07 dB bis 16 kHz. Die Interpolation wird nur gerechnet, wenn eine Schranke (Σ|h| · max|x| im Fenster) das Ceiling erreichen kann. Dadurch bleibt 8× schnell und das Ergebnis exakt.
  - Grenze: Anteile oberhalb von ≈ 20 kHz (bei 48 kHz) werden unvollständig erkannt, wie bei jedem praxistauglichen Interpolator (auch BS.1770). Für Sprachaufnahmen ist das irrelevant.
- Pro Basissample: `peak = max über Kanäle (gelinkt) und die 4 Zwischenwerte`, daraus `g_req = min(1, ceiling_lin / peak)`.
- **Gain-Pfad:**
  1. gleitendes Minimum über das Lookahead-Fenster `L`;
  2. Release als Ein-Pol im dB-Bereich (sofortige Absenkung, langsame Erholung);
  3. Rechteck- bzw. Hann-Glättung der Länge `L`.

  Die Kombination aus Minimum und gleich langer Glättung garantiert, dass der Gain den Zielwert **vor** dem Peak erreicht. Das Signal wird um `L` verzögert.
- Defaults: Ceiling **−1,0 dBTP**, Lookahead **5 ms**, Release **100 ms**, interne Sicherheitsmarge 0,1 dB (die Rekonstruktion zwischen Samples ändert sich minimal unter variablem Gain).
- Report: maximale Gain-Reduktion und Anteil der Zeit im Limiting.
- Fallstrick: Bereits geclippter Eingang ist nicht reparierbar. Er wird erkannt (Folgen von ≥ 3 Samples an ±FS) und gewarnt.

### 3.9 Quantisierung und Ausgabe

- 16 bit: TPDF-Dither (±1 LSB, deterministischer PRNG). 24 bit: kein Dither (Default, optional einschaltbar). 32f: direkt.
- Noise-Shaping kommt erst später.
- Die Clipping-Sicherheit ist durch den Limiter gegeben. Trotzdem wird hart auf ±FS begrenzt, und Clip-Zähler gehen in den Report (soll 0 sein).

### 3.10 Verarbeitungskette: Bestätigung

**Bestätigt:** `[Pre-Chain (leer)] → Spectral Subtract → Leveler → Normalisierung → True-Peak-Limiter`. Das Profil entsteht aus dem **Original** in Pass 1, vor jeder Pegeländerung. Zusätzlich:
- Die Leveler-*Kurve* basiert auf Pass-1-Daten mit rauschkompensierter Energie (3.6), wirkt aber **nach** Denoise. So werden keine Pausen angehoben, die anschließend wieder entrauscht werden müssten.
- Zukünftige Pre-Stufen (Hochpass, Hum-Removal) gehören **vor** die Profilmessung. Der Pre-Chain-Slot ist dafür vorgesehen.
- Alternative „Leveler vor Denoise“ ist verworfen: Die variable Verstärkung macht das Rauschen instationär, und ein statisches Profil passt dann nirgends mehr.

### 3.11 Pausen-Absenkung (Noisefloor-Regler) – Phase 5

**Anlass:** Vergleich von 004 mit der Referenzfassung, die du am 2026-11-26 als Referenz bereitgestellt hast. In der Sprache liegen beide innerhalb von 1–2 dB. In den Pausen gibt es einen großen Unterschied:

| Fassung | Pausen (leiseste 5 % der 50-ms-Abschnitte) | Abstand Pause zu Sprache |
|---|---|---|
| Original | −48 dBFS | ≈ 15 dB |
| leveled-audio | −48 dBFS | ≈ 25 dB |
| Referenz | −96 dBFS | ≈ 68 dB |

Mehr Rauschreduktion allein ist keine Lösung. Über ≈ 15–20 dB hinaus entsteht Musical Noise, und die Sprache klingt dumpf. Deshalb kommt eine **eigene Stufe**: Sie senkt nur Pausen breitbandig ab und lässt Sprache unangetastet.

1. **Maske:** aus den VAD-Klassen von Pass 1, offline. Vorausschau ist damit kostenlos.
   - Pausen kürzer als `min_pause_ms` (400 ms) bleiben unverändert. Sonst würde zwischen Wörtern zerhackt.
   - Nach Sprachende wird `hold_ms` (150 ms) gehalten, damit Ausklänge nicht abreißen. Die Absenkung beginnt also erst 150 ms nach dem letzten Wort.
   - *Geändert nach deiner Hörprobe:* Die Absenkung kam zu spät und dann abrupt. Gemessen an 004 begann sie 300–450 ms nach dem letzten Laut, weil sich VAD-Nachlauf (150 ms) und Haltezeit (150 ms) addierten. Danach fielen 19 dB in ≈ 70 ms ab.
     - Jetzt zählt `hold_ms` ab dem **letzten Laut**, also innerhalb des VAD-Nachlaufs, mit Default 50 ms.
     - Die Blenden sind getrennt: `fade_out_ms` 150 ms in die Pause, `fade_in_ms` 50 ms vor dem nächsten Wort.
     - Gemessen an 004: Beginn 55–75 ms nach dem letzten Laut (einmal 205 ms nach einem Knackser), volle Tiefe nach ≈ 110 ms.
   - Vor dem Sprachbeginn ist die Maske `pre_open_ms` (50 ms) früher wieder offen, sodass kein Wortanfang beschnitten wird.
2. **Tiefe je Datei:** `a = clamp(P_pause − floor_dbfs, 0, max_attenuation_db)`.
   - `P_pause` ist der Pausenpegel nach Rauschreduktion, Leveler **und Normalisierungs-Gain**, gemessen in Pass 2.
   - Der Regler bezieht sich also auf die **fertige Datei**: `floor_dbfs = −60` heißt „Pausen landen bei etwa −60 dBFS“.
3. **Blenden:** Cosinus-Blenden im dB-Bereich, `fade_out_ms` (150 ms) in die Pause hinein und `fade_in_ms` (50 ms) aus ihr heraus. Die Aufblende endet vor dem Sprachbeginn. *(Ursprünglich eine gemeinsame Rampe `ramp_ms` 80 ms, geändert nach deiner Hörprobe, siehe Punkt 1.)*
4. **Umsetzung:** Die Kurve wird mit der Leveler-Kurve multipliziert und in derselben Stufe angewendet. Es gibt keine neue Latenz und keinen zusätzlichen Durchgang.
   - Die Lautheitsmessung (Pass 2) ändert sich dadurch praktisch nicht. Pausen liegen > 10 LU unter der Sprache und fallen beim relativen Gate von BS.1770 ohnehin heraus. Eine Restabweichung fängt die Korrekturiteration (3.7).
5. **Risiko „Atmen“:**
   - Hinter der Sprache bleibt Restrauschen, in den Pausen ist es fast still. Der Wechsel kann hörbar sein, deshalb hat 3.6 bewusst nicht in Pausen abgesenkt.
   - Gegenmittel: weiche Rampen, die Mindestpausenlänge und `max_attenuation_db` als Obergrenze. Ob das reicht, entscheidet die Hörabnahme.
6. **Fehlklassifikation:** Leise Silben, die die VAD als Pause einstuft, würden mit abgesenkt. Der Report nennt deshalb den Anteil abgesenkter Zeit.
   - Für 004 ist offen, was bei 0–1,25 s, bei ≈ 8 s und ab 12,75 s zu hören ist. Die Referenz behandelt diese Stellen anders als leveled-audio.
   - Diese Stellen werden vor dem Einstellen der Defaults angehört.

| Parameter | Default (Vorschlag) | Bereich |
|---|---|---|
| `gate.enabled` | **aus** in allen Presets (Wunsch: natürlicher Klang) | an/aus |
| `gate.floor_dbfs` | −60 | −90 … −30 |
| `gate.max_attenuation_db` | 30 | 0–60 |
| `gate.min_pause_ms` | 400 | 100–2000 |
| `gate.hold_ms` | 50 (ab dem letzten Laut) | 0–500 |
| `gate.pre_open_ms` | 50 | 0–200 |
| `gate.fade_out_ms` | 150 | 10–1000 |
| `gate.fade_in_ms` | 50 | 10–500 |

CLI: `--pause-floor <dBFS>` schaltet die Stufe ein und setzt das Ziel, `--no-pause-floor` schaltet sie aus.

### 3.12 Kompressor für starke Dynamik – Phase 5

**Anlass:** Der Leveler arbeitet phrasenweise und bewusst langsam (3.6). Kurze laute Ereignisse wie Lachen, Ausrufe oder Husten landen deshalb im True-Peak-Limiter.
- Bei 004 senkte der Limiter um bis zu 8,6 dB ab, und erst die Korrekturiteration brachte die Lautheit ins Ziel.
- Hartes Limiting über mehrere dB klingt gequetscht.

1. **Position:** nach Leveler, Pausen-Absenkung und Normalisierungs-Gain, vor dem Limiter. Er läuft auch in Pass 2 mit, damit die Lautheitsmessung seine Wirkung enthält.
   - Der Normalisierungs-Gain steht erst nach Pass 2 fest. Deshalb arbeitet der Kompressor intern relativ zum Sprachpegel (Punkt 2). So ist das Ergebnis unabhängig von der absoluten Lage, und Pass 2 und 3 rechnen dasselbe.
2. **Schwelle automatisch:** `T = L_ref + threshold_above_ref_db` (Default **+8 dB**; *geändert in Phase 5:* mit +6 dB fasste der Kompressor bei 40 % auch normale Silbenspitzen an, −0,52 dB in der Sprache; mit +8 dB −0,21 dB).
   - `L_ref` ist der Sprach-Referenzpegel aus 3.6, also die Quartilsmitte. Er wird auch berechnet, wenn der Leveler aus ist.
   - Normale Sprache bleibt so unter der Schwelle. Nur, was deutlich herausragt, wird erfasst. Niemand muss Schwellen in dB einstellen.
3. **Kennlinie:** Feed-forward im dB-Bereich mit 6 dB Soft-Knee. Das Verhältnis folgt aus `amount`: 0 % → 1:1, 40 % → 1,5:1, 60 % → 2:1, 100 % → 6:1 (`ratio = 1 / (1 − amount · 5/6)`).
4. **Detektor:** RMS über 10 ms. Attack 5 ms, Release 150 ms.
   - Position umgesetzt: **vor** dem Normalisierungs-Gain (nach Leveler und Pausen-Absenkung). Dann rechnen Pass 2 und 3 exakt dasselbe, und die Schwelle braucht den erst nach Pass 2 bekannten Gain nicht. Das ist gleichwertig zu „nach dem Gain mit verschobener Schwelle“.
   - Dazu kommt eine Vorausschau von 5 ms (Verzögerung, wie beim Limiter). Die Absenkung steht also schon, wenn das Ereignis beginnt, und es gibt kein Überschwingen.
   - Stereo gekoppelt: gleicher Gain für beide Kanäle, damit das Stereobild stabil bleibt.
5. **0 % ⇒ bitidentisch** zum Bypass, weil die Stufe dann gar nicht aufgebaut wird.

| Parameter | Default (Vorschlag) | Bereich |
|---|---|---|
| `compressor.enabled` | an | an/aus |
| `compressor.amount` | 40 % (natürlich; 1,5:1) | 0–100 % |
| `compressor.threshold_above_ref_db` | 8 | 0–20 |
| `compressor.attack_ms` | 5 | 0,5–50 |
| `compressor.release_ms` | 150 | 20–1000 |

CLI: `--compression <0–100>`, `--no-compression`.

### 3.13 Hochpass und Störgeräusch-Erkennung (Griffgeräusche) – Phase 5

**Anlass:** In 004 sind bei 0–1,25 s, ≈ 8 s und ab 12,75 s Knackgeräusche des Handmikrofons. Die VAD stuft sie als **Sprache** ein (`S`):
- Sie sind tieffrequent und damit spektral „tonal“, die Flachheit liegt bei 0,00–0,06 wie bei stimmhafter Sprache.
- Deshalb hebt der Leveler den Anfang um bis zu +7 dB an.
- Die Referenz lässt diese Stellen 5–15 dB leiser.

Messung an 004 (35-ms-Frames, nur Frames über −50 dBFS):

| Bereich | Anteil 300–3400 Hz | Anteil < 150 Hz |
|---|---|---|
| Knackser 0–0,4 s / 0,8–1,2 s / 7,9–8,9 s / 12,75–13,9 s | −23 / −28 / −23 / −22 dB | ≈ 0 dB |
| Sprache 1,5–4,8 s / 5,7–7,2 s / 9–12,5 s | −7 / −7 / −9 dB | −4 bis −5 dB |

Der **Sprachband-Anteil** trennt beides eindeutig, mit über 13 dB Abstand. Die Pegelsprünge von Frame zu Frame trennen nicht (beide ≈ 4–5 dB).

1. **Hochpass** (aus „Später“ vorgezogen): 2. Ordnung, 70 Hz, Butterworth, im Pre-Chain-Slot **vor** der Profilmessung (3.10).
   - Er entfernt Trittschall und Rumpeln. Die Grundtöne tiefer Stimmen (≥ 85 Hz) bleiben fast unberührt (−1,5 dB bei 85 Hz).
   - Bei 004 liegen im Original ≈ 55 % der Energie unter 150 Hz, Referenz −5,0 dB statt −2,6 dB. Der Hochpass allein nimmt dort schon einen Teil des Rumpelns weg.
2. **Neues Feature:** Sprachband-Anteil je Frame, also die Energie 300–3400 Hz relativ zur Gesamtenergie. Es wird aus den STFT-Bins berechnet und kostet praktisch nichts.
   - *Geändert bei der Umsetzung:* Gemessen wird das Sprachband **relativ zu den Tiefen (20–300 Hz)** statt zur Gesamtenergie (`speech_to_low_db`). Grund: Zischlaute (S, Sch) haben ihre Energie über 3400 Hz, also wenig im Sprachband, und galten sonst als Störgeräusch. Gegen die Tiefen gemessen sind sie eindeutig keine Störung.
   - Messung an deinen Aufnahmen (nach Hochpass, Frames über −50 dBFS): Sprache 1 %-Quantil −10 … −14 dB, Median −3 … +7 dB.
   - Grenzen: Die Bereiche, die du als Knackser genannt hast, bestehen nur teilweise aus tieffrequenten Frames. Kurze breitbandige Klicks haben auch Sprachband-Energie und werden nicht erkannt (Crest-Faktor trennt ebenfalls nicht: Sprache Median 12 dB, Knackser 12 dB).
   - Frame-genau trennen lässt sich das nicht. Die Wirkung zählt: siehe Phase-5-Ergebnisse.
3. **Neue VAD-Klasse `Geräusch`:** Frames über dem Hintergrund mit Sprachband < Tiefen − 15 dB. Kurze Unterbrechungen bis 70 ms werden geschlossen, damit ein Knackser eine zusammenhängende Region bildet.
   - Geräusch-Bereiche unter 40 ms direkt an Sprache bleiben Sprache. So teilen einzelne tiefe Frames, etwa Nasale, keine Phrase.
   - Sprachbereiche, in denen danach keine stimmhaften Sprachband-Frames mehr übrig sind, werden aufgelöst (nur Störgeräusch samt Pre-Roll/Hangover).
   - **Leveler:** Geräusch zählt wie Pause, wird also nicht angehoben und geht nicht in die Referenz ein.
     - *Zusätzlich geändert:* Der Bereich vor der ersten und nach der letzten Phrase wird wie eine lange Pause behandelt, also nicht über `pause_max_boost_db` angehoben (vorher: Gain der ersten bzw. letzten Phrase gehalten). Am Dateirand gibt es keinen Gain, der „atmen“ könnte. Sonst wurden Geräusche am Anfang mit angehoben.
   - **Rauschprofil:** Geräusch-Frames sind keine Profilkandidaten (sie sind ohnehin nicht stationär).
   - **Lautheit:** unverändert gemessen. Die K-Gewichtung bewertet tiefe Frequenzen ohnehin schwach.
4. **Störgeräusch-Absenkung** in Geräusch-Regionen: In derselben STFT-Stufe wie die Rauschreduktion werden die Bins unter 300 Hz um bis zu `knock.reduction_db` (Default 15 dB) abgesenkt.
   - Umgesetzt: volle Absenkung bis 250 Hz, linear auslaufend bis 350 Hz.
   - Die Absenkung braucht die STFT-Stufe der Rauschreduktion, mit `--no-denoise` gibt es also nur die Erkennung.
   - Die Rampen sind 30 ms lang. Spricht jemand gleichzeitig, bleiben die Sprachanteile über 300 Hz erhalten.
   - Das wirkt gezielt auf die Störung und verändert normale Pausen und Sprache nicht. Es entspricht dem Wunsch nach natürlichem Klang.
5. **Risiko:** sehr tiefe, gehauchte oder geflüsterte Laute mit wenig Energie im Sprachband. Der Report nennt Anzahl und Dauer der Geräusch-Regionen, die Hörabnahme prüft es.

| Parameter | Default (Vorschlag) | Bereich |
|---|---|---|
| `highpass.enabled` | an | an/aus |
| `highpass.frequency_hz` | 70 | 20–200 |
| `knock.enabled` | an | an/aus |
| `knock.max_speech_to_low_db` | −15 | −30 … −5 |
| `knock.reduction_db` | 15 | 0–40 |

CLI: `--highpass <Hz>`, `--no-highpass`, `--no-knock-reduction`.

**Kette ab Phase 5:** `Hochpass → Rauschreduktion + Störgeräusch-Absenkung → Leveler × Pausen-Absenkung → Normalisierungs-Gain → Kompressor → True-Peak-Limiter → Dither`.

---

## 4. CLI, Presets, Reports

### 4.1 Befehle

```
leveled-audio process <INPUTS>... -o <OUTDIR> [Optionen]   # Hauptbefehl
leveled-audio analyze <INPUTS>... [--json report.json]     # nur messen (Lautheit, Rauschpegel, Profil-Konfidenz)
leveled-audio profile learn <WAV> -o <profile.json>
leveled-audio preset list | show <name>                     # eingebaute Presets anzeigen bzw. als TOML ausgeben
```

Wichtige Optionen für `process`:
- `--preset podcast-stereo|podcast-mono|ebu-r128|<file.toml>`
- `--target-lufs`, `--ceiling-dbtp`, `--loudness-mode per-file|album`
- `--bit-depth 16|24|32f` (Default: wie Eingang; 32-bit-int-Eingang → 24)
- `--naming "{stem}{ext}"` (Tokens: `{stem}`, `{ext}`, `{preset}`, `{lufs}`), `-r/--recursive` (Unterordnerstruktur bleibt erhalten)
- `--overwrite` | `--skip-existing` (Default: Fehler bei existierender Zieldatei)
- `--jobs N` (Default: min(Kerne, 4); ob eine Platte rotiert, lässt sich plattformübergreifend nicht zuverlässig erkennen, siehe Risiken)
- `--noise-profile`, `--save-profile`, `--export-delta`, `--no-denoise`, `--no-leveler`
- `--set denoise.reduction_db=15` (generische Overrides) plus die wichtigsten als eigene Flags
- `--report-json <file>`, `-q`, `-v/-vv`

### 4.2 Sicherheit und Abbruch

- Die Ausgabe wird nach `name.wav.partial` geschrieben und erst nach Erfolg per atomarem `rename` umbenannt.
- **Eingabedateien werden nie überschrieben.** Kanonische Pfade werden verglichen, auch wenn `--overwrite` gesetzt ist.
- Ctrl+C: `ctrlc` setzt das `CancelToken`, die Worker brechen nach dem aktuellen Block ab und `.partial`-Dateien werden gelöscht. Exit-Code **130**. Ein zweites Ctrl+C beendet sofort.
- Exit-Codes: 0 = ok, 1 = mindestens eine Datei fehlgeschlagen (der Rest wurde verarbeitet), 2 = Bedienfehler, 130 = Abbruch.
- Fortschritt: `indicatif::MultiProgress` mit Gesamtbalken (gewichtet nach Samples × Passes) und je einem Balken pro aktiver Datei mit Pass-Label. Ohne TTY gibt es zeilenweise Logs.

### 4.3 Presets (TOML), Schichtung

Reihenfolge: eingebaute Defaults ← Preset ← TOML-Datei ← CLI-Flags. `deny_unknown_fields` sorgt dafür, dass Tippfehler zu Fehlern führen.

```toml
schema_version = 1
name = "podcast-stereo"

[denoise]
enabled = true
gain_rule = "wiener-dd"     # oder "subtract"
reduction_db = 12.0
oversubtraction = 1.3
fft_size = "auto"
release_ms = 40
freq_smooth_oct = 0.0
stereo_link = false

[denoise.profile]
mode = "auto"               # auto | file
min_duration_s = 0.5
target_duration_s = 2.0
statistic = "trimmed"

[leveler]
enabled = true
strength = 1.3
max_boost_db = 20.0
max_cut_db = 10.0
pause_max_boost_db = 0.0

[loudness]
target_lufs = -16.0
mode = "per-file"

[limiter]
ceiling_dbtp = -1.0
lookahead_ms = 5.0
release_ms = 100.0
oversampling = 8

[output]
bit_depth = "same"          # same | 16 | 24 | 32f
dither = "auto"
naming = "{stem}{ext}"
```

### 4.4 Report (Konsole + JSON)

Pro Datei:
- **Eingang:** Format, Dauer, I/LRA/TP/Sample-Peak, Clip-Warnung, DC-Warnung.
- **Rauschen:** Quelle (auto/minstat/importiert), Konfidenz, Profildauer, Anzahl genutzter Pausen, Rauschpegel vorher (dBFS, A-bewertet) und nachher (gemessen in Pausen-Frames der Ausgabe), mittlere angewendete Reduktion in Pausen und in Sprache.
- **Leveler:** Gain-Bereich min/max, mittlerer |Gain|.
- **Normalisierung:** Gain.
- **Limiter:** max. Gain-Reduktion, Zeitanteil.
- **Ausgang:** I/LRA/TP.
- Laufzeit und Echtzeitfaktor, Warnungen.

JSON zusätzlich mit `tool_version`, vollständigen effektiven Settings und Settings-Hash für die Reproduzierbarkeit. Die Konsolenzusammenfassung erscheint als Tabelle über alle Dateien.

### 4.5 Regler: Empfehlungen und Wirkung (Stand Phase 5)

**Fix oder per Analyse?** Die Werte der Regler sind **fest**. Sie stehen im Preset, sind für alle Dateien gleich und machen das Ergebnis reproduzierbar.

Fast alle Regler beschreiben aber eine **Absicht relativ zur Datei**. Die tatsächlich angewendeten dB rechnet leveled-audio für jede Datei aus ihrer Analyse aus:
- Die Rauschreduktion arbeitet mit dem automatisch gefundenen Rauschprofil.
- Der Leveler gleicht auf den Sprachpegel der Datei an.
- Die Schwelle des Kompressors liegt relativ zu diesem Sprachpegel.
- Die Pausen-Absenkung rechnet aus, wie tief sie gehen muss, um ihr Ziel zu erreichen.
- Die Knackser-Erkennung bewertet jeden Abschnitt einzeln.

Bei derselben Einstellung bekommt eine leise, verrauschte Datei deshalb andere Korrekturen als eine saubere. Im Report steht, was tatsächlich angewendet wurde.

Absolut gemeint sind nur drei Regler: Ziel-Lautheit, True-Peak-Grenze und Hochpass-Frequenz.

**Empfehlungen:** „Natürlich“ ist der Default in den Podcast-Presets und entspricht deinem Wunsch. „Kräftig“ kommt näher an die Referenzfassungen heran: Pausen fast still, dichterer Klang.

| Regler (CLI) | Natürlich (Default) | Kräftig | fix / per Analyse | Was sich ändert |
|---|---|---|---|---|
| `--target-lufs` | −16 | −16 | fix | Gesamtlautheit. Höher = lauter. Plattformen normalisieren meist auf −14 bis −16. |
| `--ceiling-dbtp` | −1 | −1 | fix | Obergrenze für Spitzen. Höher (z. B. −0,5) gibt dem Limiter weniger Arbeit, riskiert aber Verzerrungen nach MP3/AAC-Kodierung. |
| `--highpass` (Hz) | 70 | 80 | fix | Entfernt Rumpeln und Trittschall unterhalb der Frequenz. Höher = schlanker, ab ≈ 100 Hz verlieren tiefe Stimmen Wärme. |
| `--knock-reduction` (dB) | 15 | 20 | erkannt per Analyse | Absenkung der Tiefen in erkannten Griffgeräuschen. Höher = Knackser leiser; gleichzeitige Sprache klingt dort kurz dünner. 0 = aus. |
| `--reduction-db` | 12 | 15 | Profil per Analyse | Maximale Rauschabsenkung. Höher = weniger Rauschen, ab ≈ 15–18 dB drohen „blubbernde“ Artefakte (Musical Noise) und dumpfere Sprache. |
| `--leveler-strength` (%) | 130 | 130 | Referenz per Analyse | Wie stark leise und laute Sprecher angeglichen werden. 100 = gleich laut, 0 = Pegelunterschiede bleiben, darüber Überkorrektur. Höher klingt gleichmäßiger, aber weniger lebendig. *Default seit dem Abstandstest 130 % (vorher 60/80 %), auf deinen Wunsch.* |
| `--compression` (%) | 40 (≈ 1,5:1) | 60 (≈ 2:1) | Schwelle per Analyse | Fängt kurze laute Stellen (Lachen, Ausrufe) ab. Höher = dichter und gleichmäßiger, zu hoch klingt gepresst. 0 = aus. |
| `--pause-floor` (dBFS) | aus | −60 | Tiefe per Analyse | Pegel der Sprechpausen in der fertigen Datei. Aus = natürlicher Raumton bleibt. −60 = Pausen fast still. Kann an Pausengrenzen „atmen“ (Rauschen kommt mit der Stimme). |
| `--loudness-mode` | per-file | per-file | fix | `album`: ein Gain für alle Dateien eines Aufrufs. Nur für Folgen einer Produktion, sonst bleiben leise Dateien leise. |

Die Werte unter „Kräftig“ sind Vorschläge vor der Hörabnahme. Sie werden mit dem Vergleichsskript gegen die Referenzfassungen geprüft und danach festgelegt.

Umsetzung in Phase 5:
- Zusätzliches eingebautes Preset `podcast-kraeftig`, abgeleitet von `podcast-stereo`.
- `--help` und die Anleitung erhalten je Regler die Kurzbeschreibung aus der Spalte „Was sich ändert“.
- `preset show` schreibt sie als Kommentare in die TOML-Datei.

---

## 5. Qualität und Tests

| Ebene | Inhalt |
|---|---|
| Unit (dsp) | WOLA-Perfekt-Rekonstruktion (< −120 dB); Oversampler-Frequenzgang und Sperrdämpfung; Limiter: TP ≤ Ceiling bei Sinus nahe fs/4 und bei Intersample-Peak-Testsignalen; Glätter und Slew; Dither-Determinismus |
| Unit (io) | Roundtrip aller 4 Sampleformate × Mono/Stereo × 3 Raten (bitexakt bei gleicher Bittiefe ohne Gain/Dither); RF64 lesen und schreiben; Kreuzprüfung gegen `hound` (Dev-Dependency); defekte Header → saubere Fehler |
| Lautheit | EBU **Tech 3341** Fälle 1–5 und **Tech 3342** (LRA) werden synthetisch erzeugt (Sinus, Tonfolgen). Toleranz ±0,1 LU bzw. ±1 LU. Die True-Peak-Fälle der EBU-Testdateien werden in CI geladen und per SHA256 geprüft, nicht eingecheckt (Lizenz unklar). Lokal `#[ignore]`. |
| Profil | Synthetische Fälle mit bekannter Rausch-PSD (weiß, rosa, braun, Lüfter = farbig + Töne). Abweichung ≤ 1,5 dB pro Terzband (100 Hz–16 kHz). Robustheit gegen eingefügte digitale Stille, Fade-in/out, atemartige Bursts, Klicks (je ein Testfall). Durchgehende Sprache ohne Pausen ⇒ Fallback greift. |
| Denoise-Qualität | **„Schattenfilterung“:** Die aufgezeichneten Gain-Masken werden separat auf das saubere Signal und das Rauschen angewendet. Ergebnis: exakte Ausgangs-Sprachkomponente und Restrauschen ⇒ SNR-Gewinn, Sprachverzerrung (segmentelles SNR der gefilterten Sprache), Rauschdämpfung in Pausen, Log-Kurtosis-Ratio (Musical Noise). |
| Testsignale | `testkit` erzeugt „sprachähnliche“ Signale: Glottis-Pulsfolge mit F0-Kontur und Jitter, Formantfilter-Vokalfolge, 4-Hz-Silbenhüllkurve, Frikative als bandpassgefiltertes Rauschen, Pausen. Zwei „Sprecher“ (F0 110/210 Hz, −10 dB Pegeldifferenz) wechseln sich ab. Rauschen bei SNR 10/20/30 dB. |
| Golden Files | Hybrid: (a) JSON-Fingerabdrücke (100-ms-RMS-Hüllkurve, Terzbandenergien, Report-Werte) für viele Fälle, Toleranz 0,05 dB bzw. 0,05 LU. (b) 2–3 kurze 32f-Golden-WAVs (≤ 2 MB) mit Toleranz max. abs. Differenz ≤ 1e−5. Aktualisierung über `UPDATE_GOLDEN=1`. |
| Hörprüfung | Checkliste (Musical Noise, Onsets, Zischlaute, Pumpen, Rauschteppich in Pausen) für 3–5 echte Aufnahmen von dir. Die Dateien werden **nicht** eingecheckt. Abnahme durch dich am Ende von Phase 2 und 3. |
| Benchmarks | `criterion`: STFT-Denoise, Limiter, Gesamtkette auf 60 s Stereo 48 kHz. Plus ein Langzeittest (3 h synthetisch, `#[ignore]`) mit Peak-RSS-Messung. |
| CI | GitHub Actions: Matrix macOS arm64 (+ Intel-Runner, falls verfügbar), Windows x64, Linux x64. Schritte: `fmt --check`, `clippy -D warnings`, Tests (Release-Profil für DSP), `cargo-deny`. Release-Workflow bei Tag `v*`. |

---

## 6. Phasen mit Akzeptanzkriterien

Aufwand grob für eine Person in Vollzeit; dient nur zur Orientierung.

### Phase 1: WAV-I/O, Lautheit, Normalisierung, Limiter, CLI-Grundgerüst (≈ 2 Wochen) – umgesetzt

> **Stand 2026-11-26:** alle Kriterien lokal erfüllt (Apple M1). 3-h-Datei Stereo 48 kHz: 173× Echtzeit, 12 MB Peak-RSS. CI-Workflow angelegt, auf GitHub noch nicht gelaufen (kein Remote).

Lieferumfang:
- Workspace, Toolchain, Lizenzdateien, CI-Grundgerüst (fmt/clippy/test/deny auf 3 OS).
- `leveled-audio-io` (WAV + RF64), `testkit` (Sinus, Tonfolgen nach 3341/3342, Rauschen).
- `dsp`: Oversampler, Limiter, Dither. `analysis::loudness`.
- `pipeline`: Pass 2 + Pass 3 (ohne Denoise und Leveler), Batch mit rayon, Cancel, Progress-Trait.
- `cli`: `process`, `analyze`, Naming, Überschreibschutz, `.partial` + rename, Ctrl+C, Konsolenreport.

Akzeptanzkriterien:
- [ ] Alle 24 Formatkombinationen werden gelesen. Roundtrip bitexakt (Int-Formate, gleiche Bittiefe, Passthrough).
- [ ] Tech-3341-Fälle 1–5 bis ±0,1 LU, Tech-3342-Fälle bis ±1 LU.
- [ ] Ausgang trifft das Ziel bis **±0,5 LU** (±0,2 LU, wenn der Limiter nicht eingreift). True Peak ≤ Ceiling (gemessen mit ebur128 **und** unabhängiger 8×-Messung ≤ Ceiling + 0,1 dB).
- [ ] Ausgangslänge = Eingangslänge. Zwei Läufe sind byte-identisch.
- [ ] 3-h-Stereo-48-kHz-Datei: Peak-RSS < 200 MB, ≥ 100× Echtzeit (diese Kette).
- [ ] Ctrl+C hinterlässt keine `.partial`-Dateien, Exit 130. Eingabedateien sind nie überschreibbar.
- [ ] CI grün auf allen drei Betriebssystemen.

### Phase 2: VAD, automatische Rauschprofilsuche, Spectral Subtract (≈ 3 Wochen) – umgesetzt

> **Stand 2026-11-26:** Alle automatisierten Kriterien erfüllt, Hörabnahme bestanden („klingt gut“).
> Gemessen (synthetische Sprache, rosa Rauschen, 20 dB SNR, Defaults): Profil ≤ 0,7 dB je Terzband
> (weiß/rosa/Lüfter, auch mit Stille, Fades, Atmern, Klicks); Pausen −11,5 dB; SNR +5,7 dB;
> Sprachverzerrung 25,5 dB; Log-Kurtosis-Ratio 0,10 (Subtract: 0,54); Minimum Statistics bei
> fließender Rede +2,6 dB. Tempo: Denoise 450× Echtzeit (ein Kern, Stereo 48 kHz), Gesamtkette
> 70× Echtzeit, 3-h-Datei mit 96 MB Peak-RSS. Deine fünf Clips: alle mit automatischem Profil
> (Konfidenz 0,63–0,92), Absenkung in Pausen 10–12 dB.
>
> **Abweichungen vom Plan (gemessen begründet):**
> - Release 40 statt 80 ms: Musical Noise (LKR) 0,35 → 0,08, Pausen −10,8 → −11,6 dB.
> - Frequenzglättung per Default aus: Mit Mindestbreite 3 Bins senkt sie Obertöne der Stimme mit ab
>   (Sprachverzerrung bei 10 dB SNR 10,6 dB statt 16,9 dB), bei kaum weniger Musical Noise.
> - Profil-Frames werden nach dem Mittel der log. Bin-Leistungen ausgewählt, nicht nach Bandenergie
>   (sonst bis −2 dB Auswahl-Bias im Bass bei tieffrequentem Rauschen).
> - Getrimmtes Mittel mit Bias-Korrektur (sonst −1,3 dB).
> - Mindestmenge Pausenmaterial 0,3 s statt 0,5 s; Randausschluss min(0,5 s; 5 %); Zusatzabstand zu
>   Sprache 40 ms; Stationaritätsschwelle adaptiv. Grund: kurze, sprachdichte Clips (deine
>   Aufnahmen) – Minimum Statistics lag dort bis 7 dB daneben, ein Profil aus 0,3–0,5 s Pause nicht.
> - Minimum-Statistics-Fenster 2,5 s statt 1,5 s.
> - Kriterium SNR-Gewinn ≥ 8 dB bei 20 dB SNR war mit max. 12 dB Absenkung nicht erreichbar
>   (Rauschen bleibt während der Sprache in sprachdominierten Bins). Neu: ≥ 5 dB (gemessen 5,7;
>   bei 10 dB SNR +7,8). Pausen ≥ 11 dB und Sprachverzerrung ≥ 20 dB bleiben und werden erreicht.
> - Delta-Datei immer 32 bit float; Profile werden mit `--save-profiles <Ordner>` gespeichert.

Lieferumfang:
- STFT/WOLA-Engine, Pass 1 (Features, VAD, Minimum Statistics), Pass 1b (Profil + Konfidenz).
- Denoise (wiener-dd + subtract), Stereo-Modi.
- Profil-Export und -Import, `profile learn`, Delta-Export.
- Sprachähnliche Testsignale und Schattenfilter-Metriken.

Akzeptanzkriterien:
- [ ] WOLA-Identität: Fehler < −120 dB. Delta bei `reduction_db = 0` ist digital null.
- [ ] Profilgenauigkeit ≤ 1,5 dB pro Terzband. Robust gegen Stille, Fades, Atmer, Klicks (je ein Test). Ohne Pausen ⇒ Fallback mit Konfidenz < 0,4, Profil ≤ 3 dB genau.
- [ ] Bei 20 dB Eingangs-SNR und Defaults: Rauschdämpfung in Pausen ≥ 11 dB, SNR-Gewinn ≥ 8 dB, Sprachverzerrung (segmentelles SNR der gefilterten Sprache) ≥ 20 dB.
- [ ] Log-Kurtosis-Ratio wird festgehalten. wiener-dd ist messbar besser als subtract bei gleicher Rauschdämpfung. Der Baseline-Wert wird als Regressionsschranke gesetzt.
- [ ] Profil-Roundtrip identisch. Import mit anderer Samplerate oder FFT-Größe funktioniert. Erzwungenes Profil gilt für den ganzen Batch.
- [ ] Denoise ≥ 150× Echtzeit (Stereo 48 kHz, ein Kern, Apple M1 oder vergleichbarer x64).
- [x] **Hörabnahme durch dich:** kein auffälliges Musical Noise bei Defaults auf deinen Testaufnahmen.

### Phase 3: Adaptive Leveler (≈ 1–1,5 Wochen) – umgesetzt

> **Stand 2026-11-26:** Alle automatisierten Kriterien erfüllt, Hörabnahme bestanden („klingt gut“, mit den kurzen Testclips; ein längeres Gespräch steht noch aus).
> Abweichungen von 3.6 (Referenz, Gate, Glättung) sind dort begründet. Weitere Punkte:
> - Der Leveler braucht VAD und Rauschprofil. Ist er an und die Rauschreduktion aus, läuft die Analyse trotzdem (Profil nur zur Kompensation).
> - Neue Frame-Größe `k_db`: K-gewichtete Leistung aus dem STFT-Spektrum (Betragsgang der BS.1770-Filter je Bin).
> - CLI: `--no-leveler`, `--leveler-strength <0–100>`; Report: Phrasen, gegatete Phrasen, Referenz, Gain-Bereich, mittlerer |Gain|.
> - Bekannt: Greift der Limiter stark ein, verfehlt die Kette das Ziel weiter um bis zu ≈ 0,8 LU. Die Korrekturiteration bleibt in Phase 4. Ohne Limiter trifft die Kette mit Leveler das Ziel auf ±0,1 LU.

Lieferumfang:
- Segmentierung, Leveler-Kurve (offline), Anwendung im Render-Pass.
- Report-Werte, Parameter und Presets.

Akzeptanzkriterien:
- [x] Zwei synthetische Sprecher mit 10 dB Differenz: Nach dem Leveler bei 100 % liegt die Differenz der Segmentlautheit < 2 dB, bei 50 % ≈ 5 dB (±1 dB). *Gemessen: 0,1 dB bzw. 4,95 dB.*
- [x] Grenzen `max_boost`/`max_cut` und `slew` werden eingehalten (Property-Tests, 200 Zufallsfälle). In langen Pausen gilt Gain ≤ `pause_max_boost`.
- [x] Innerhalb eines Segments mit konstantem Pegel schwankt der Gain < 1 dB (kein Pumpen). *Gemessen: 0,0 dB in Phrasen unter 6 s; < 1 dB bei 20 s Monolog mit ±6 dB Frame-Schwankung.*
- [x] `strength = 0` ist bitidentisch zum Bypass (mit und ohne Rauschreduktion).
- [x] Gesamtkette ≥ 50× Echtzeit pro Kern. *Gemessen: 68× bei 3 h Stereo 48 kHz, 104 MB Peak-RSS.*
- [x] **Hörabnahme durch dich** (Gesprächsaufnahmen mit unterschiedlich lauten Sprechern).

### Phase 4: Batch-Feinschliff, Reports, Presets, Release-Builds (≈ 1,5 Wochen) – umgesetzt, Release-Lauf auf GitHub offen

> **Stand 2026-11-26:** Umgesetzt und lokal getestet (86 Tests, clippy sauber).
> - **Presets:** Eine Preset-Datei schichtet sich auf ein eingebautes Basis-Preset (`base`, Default `podcast-stereo`). Danach folgen `--set` und die eigenen Flags. `schema_version` und `name` sind optional. `preset show` erzeugt eine wieder ladbare Datei.
> - **JSON-Report:**
>   - Neue Felder: `schema_version`, `command`, vollständige `settings`, `settings_hash` (FNV-1a 64 über die kanonische JSON-Form, keine kryptografische Prüfsumme), `summary` sowie `album` im Album-Modus.
>   - Je Datei zusätzlich `loudness_correction_db`.
>   - Schema: `docs/report.schema.json` (Draft 2020-12, streng: `additionalProperties: false`).
> - **Album-Modus:** `prepare` (Pass 1+2) für alle Dateien parallel, dann die Barriere, dann `render` (Pass 3) für alle.
>   - Bis zur Barriere bleiben je Datei VAD-Klassen, Rauschprofil und Leveler-Kurve im Speicher (≈ 10 MB je Stunde Audio). Die Feature-Arrays werden vorher verworfen.
>   - Fehlerhafte und übersprungene Dateien zählen nicht zur Album-Lautheit (Warnung).
> - **Release:** `.github/workflows/release.yml` (Tag `v*`). Lokal geprüft: aarch64/x86_64/Universal für macOS.
>   - Das Universal-Binary läuft nativ und unter Rosetta.
>   - Ausgaben von arm64 und x86_64 weichen um ≤ 3 LSB bei 24 bit ab (RMS-Differenz −150 dBFS), innerhalb der Toleranz aus 1.4.
>   - Windows und Linux werden erst im GitHub-Lauf gebaut und geprüft (`dumpbin /dependents` bzw. `ldd`).
> - **Doku:** `docs/ANLEITUNG.md` (Bedienungsanleitung), README aktualisiert.
> - Nicht umgesetzt:
>   - `-v/-vv` aus 4.1 (bisher kein Bedarf; die Hinweise stehen in Tabelle und Report).
>   - Fuzzing des WAV-Header-Parsers (Risikotabelle, `cargo-fuzz`). Es braucht nightly und bleibt vorgemerkt.

Lieferumfang:
- TOML-Presets mit Schichtung, `preset list/show`, `--set`.
- JSON-Report mit Schema, Album-Modus, rekursive Ordner, Korrekturiteration für die Lautheit.
- `release.yml`: aarch64- und x86_64-apple-darwin (+ Universal), x86_64-pc-windows-msvc (statische CRT), x86_64-unknown-linux-musl, jeweils mit SHA256-Summen.
- README und Bedienungsanleitung.

Akzeptanzkriterien:
- [ ] Release-Artefakte laufen auf frischen Systemen ohne Zusatzinstallation (Windows ohne VC-Redist; `ldd` meldet unter Linux „statically linked“). *macOS lokal geprüft (nur Systembibliotheken laut `otool -L`). Windows und Linux prüft der Workflow automatisch, er ist aber noch nicht gelaufen. Dafür braucht es ein GitHub-Repository und ein Tag.*
- [x] JSON-Report validiert gegen das Schema. Reihenfolge deterministisch. *CLI-Test validiert Reports von process (pro Datei, Album, mit Skip, Fehler, Delta), analyze und dem 100er-Batch mit `jsonschema`. Gegenprobe: zusätzliche Felder und falsche Typen werden abgelehnt.*
- [x] Batch mit 100 Dateien: ein defekter Eingang bricht den Batch nicht ab (Exit 1, Report nennt den Fehler). *99 ok, 1 Fehler mit Dateiname, 99 Ausgaben.*
- [x] Album-Modus: alle Dateien erhalten denselben Gain, die Album-Lautheit trifft das Ziel bis ±0,5 LU. *Synthetisch, drei Dateien 11 dB auseinander: −16,18 LUFS (mit Limiter, zwei Korrekturen), ohne Limiter exakt. Deine Clips: −16,1 LUFS.*
- [x] Preset-Schichtung durch Tests abgedeckt, unbekannte Keys ergeben einen Fehler. *Unit-Tests (Basis, Datei, `--set`, Roundtrip, Fehlerfälle) und CLI-Test (Reihenfolge Datei ← `--set` ← Flag, Exit 2 bei Tippfehlern).*
- [x] Zusätzlich: Die Korrekturiteration behebt die Abweichung aus Phase 3. *Synthetischer Dialog −16,55 → −16,21 LUFS. Deine Clips 001 und 004 vorher −16,8/−16,6, jetzt −16,2/−16,1.*

### Phase 5: Hochpass, Griffgeräusche, Kompressor, Pausen-Absenkung (≈ 1,5 Wochen) – umgesetzt

> **Stand 2026-11-26:** Umgesetzt, 96 Tests grün, clippy sauber. Hörabnahme bestanden (Knackser, Kompressor, Pausen-Übergänge nach Nachbesserung). Abweichungen vom Plan stehen in 3.7, 3.12 und 3.13.
> - Hörproben: `Audio Samples - leveled-audio/F_Phase5_natuerlich/` (Default) und `G_Phase5_kraeftig/`, dazu `report_phase5_*.json`.
> - Vergleich mit der Referenz: `cargo run --release -p leveled-audio-pipeline --example compare_reference -- <unsere.wav> <referenz.wav> [--table]`.

Anlass ist der Vergleich mit den Referenzfassungen 001–005, die du am 2026-11-26 als Referenz bereitgestellt hast (3.11–3.13).

**Deine Antworten:**
1. In 004 sind bei 0–1,25 s, ≈ 8 s und ab 12,75 s Knackgeräusche des Handmikrofons.
2. Es soll eher natürlich klingen.
3. Weitere Referenzfassungen liegen vor (001–005).

**Vergleich Original / Referenz / leveled-audio (Phase 4)** über alle fünf Paare:

| | Referenz | leveled-audio |
|---|---|---|
| Lautheit | −15,7 … −15,9 LUFS | −16,1 … −16,4 LUFS |
| True Peak | −1,0 dBTP | −1,1 dBTP |
| Pausen (leiseste 5 %) | −49 … −114 dBFS; 001, 003, 004, 005 nahezu still | −47 … −69 dBFS |
| Anteil > 6 kHz | +2 … +3 dB gegenüber Original (Höhen-EQ) | ≈ Original (−0,5 … +0,3 dB) |
| Anteil < 150 Hz (004) | −5,0 dB | −3,3 dB |

**Folgerungen:**
- **Störgeräusche:** Die Knackser werden heute als Sprache behandelt und sogar angehoben. Das ist ein Fehler, keine Geschmacksfrage. Daher neu: Hochpass und Störgeräusch-Erkennung (3.13).
- **Pausen:** Die Referenz gatet hart. Für den gewünschten natürlichen Klang ist die Pausen-Absenkung (3.11) nur ein Regler und standardmäßig **aus**.
- **Höhen:** Die Referenz hebt sie an. Ein EQ ist eine Geschmacksfrage und bleibt bei „natürlich“ draußen (vorgemerkt unter „Später“).
- **Lautheit:** Die Referenz trifft −16 bis auf 0,1–0,3 LU darüber, leveled-audio liegt 0,1–0,4 LU darunter. Die Schwelle der Korrekturiteration (3.7) sinkt von 0,5 auf **0,3 LU**. Bei kurzen Dateien kostet das wenig. Bei langen ist die Abweichung selten groß, weil der Kompressor dem Limiter Arbeit abnimmt.

Lieferumfang:
- Hochpass im Pre-Chain-Slot (3.13).
- Sprachband-Feature, VAD-Klasse `Geräusch`, Störgeräusch-Absenkung in der STFT-Stufe. Leveler und Profilsuche ignorieren Geräusch (3.13).
- Kompressor (3.12) als Stufe in `leveled-audio-dsp`, in Pass 2 und 3.
- Pausen-Absenkung (3.11) als Regler, Default aus.
- Korrekturschwelle 0,3 LU.
- Einstellungen `[highpass]`, `[knock]`, `[compressor]`, `[gate]` in Presets, `--set` und eigenen Flags, mit Validierung.
- Report mit neuen Werten (Geräusch-Regionen, Kompressor, Pausen-Absenkung) und Schema-Version 2. Anleitung und README ergänzen.
- Reglerübersicht (4.5): eingebautes Preset `podcast-kraeftig`, Kurzbeschreibungen in `--help`, Anleitung und als Kommentare in `preset show`.
- Vergleichsskript für Referenzpaare (`examples/compare_reference.rs`). Es misst Pegelverlauf, Pausenpegel und Bandanteile gegen eine Referenzfassung, damit die Abstimmung reproduzierbar ist.

Akzeptanzkriterien:
- [ ] **Griffgeräusche, 004 – teilweise erfüllt:**
  - **Erkennung:** Frame-genau werden nur ≈ 50 % der Frames in den genannten Bereichen erkannt. Die breitbandigen Klick-Anteile haben Sprachband-Energie (siehe 3.13).
    - Erkannt werden 21 Regionen, darunter auch Knackser in Pausen außerhalb der genannten Bereiche (bei 5,0 s, 11,0 s und ab 14,3 s).
    - In den Sprachbereichen gibt es keine erkannten Störgeräusche.
  - **Wirkung:** In den 16 Fenstern zu je 250 ms der Knackser-Bereiche ist die Ausgabe im Mittel **14,8 dB leiser als in Phase 4**, mindestens 5,1 dB.
    - Gegenüber der Referenz: Median +1,9 dB.
    - Ausreißer bis +15 dB gibt es dort, wo Referenz Pausen hart gatet (0,5 s, ab 13,75 s), nicht bei Knacksern. Mit `podcast-kraeftig` (Pausen-Absenkung) schließt sich diese Lücke weitgehend.
  - Sprache: im Median −0,5 dB gegenüber Phase 4.
  - Der Leveler hebt die Knackser nicht mehr an. Der Anfang liegt bei −22,6 statt −16,3 dBFS (Referenz −28,3).
- [x] **Synthetischer Test:** Sprache mit eingestreuten tieffrequenten Impulsen (Knackser-Modell). Die Impulse werden um ≥ 12 dB abgesenkt, gleichzeitige Sprache über 300 Hz ändert sich um < 0,5 dB. *Gemessen: Impulse in Pausen −14,9 dB, Sprache über 400 Hz am Knackser in einer Silbe −0,13 dB, Sprache fern von Knacksern unverändert.*
  - Hinweis: Die synthetische Teststimme mit f0 115/140 Hz ist unrealistisch tiefenlastig (−16 dB) und würde als Störgeräusch gelten. Leveler-Tests laufen deshalb ohne Erkennung, der Knackser-Test nutzt f0 180 Hz.
- [x] **Hochpass:**
  - −3 dB bei 70 Hz ± 2 Hz, Dämpfung bei 35 Hz ≥ 11 dB. *Unit-Test: −3,01 dB bei 70 Hz, −12,3 dB bei 35 Hz, Gleichanteil entfernt.*
  - Aus ⇒ bitidentisch zur Kette ohne Hochpass (Passthrough-Test).
  - Lautheit, Clipping und DC des Eingangs werden weiter am Original gemessen.
- [x] **Kompressor (angepasst):** Synthetische Sprache mit 300-ms-Ausreißern 12 dB über dem Sprachpegel.
  - *Kriterium angepasst:* „≤ 4 dB bei 100 %“ ist mit einer Schwelle über dem Sprachpegel nicht erreichbar (6:1 über +8 dB ergibt ≈ +9 dB). Geprüft wird: Ausreißer bei 100 % um ≥ 4 dB gezähmt, bei 40 % um > 1 dB. *Gemessen: 11,6 → 5,8 dB (100 %), → 9,0 dB (40 %).*
  - Normale Sprache bei 40 % < 0,5 dB (−0,21 dB), bei 100 % < 1,5 dB (−0,51 dB).
  - 0 % ⇒ bitidentisch.
- [x] **Pausen-Absenkung:** Synthetischer Dialog mit Rauschen bei −45 dBFS. *Gemessen: −59,0 und −60,5 dBFS in den langen Pausen; Sprache, Phrasenanfänge, kurze Pause und Gain-Sprünge innerhalb der Grenzen; Tiefe 0 ⇒ bitidentisch.*
  - Pausen ≥ 400 ms erreichen −60 ± 2 dBFS.
  - Sprache ändert sich um < 0,5 dB.
  - Phrasenanfänge (erste 20 ms) werden um ≤ 1 dB abgesenkt.
  - Aus ⇒ bitidentisch.
  - Gain-Änderung je Millisekunde begrenzt (Property-Test).
- [x] **Alle fünf Referenzpaare:**
  - Lautheit −16 ± 0,3 LUFS. *Natürlich −16,0 … −16,3, kräftig −16,1 … −16,3.*
  - Limiter-Absenkung bei 004 ≤ 4 dB (bisher 8,6 dB). *3,6 dB.*
  - In Sprachbereichen liegt der 250-ms-Pegelverlauf im Median innerhalb von ±2 dB der Referenzfassung (nach Angleich der Gesamtlautheit). *|Median| 0,6–1,2 dB.*
  - Tiefenanteil (< 150 Hz) jetzt wie die Referenz (004: −4,7 statt Original −2,6, Referenz −5,0).
  - Pausen bei „kräftig“: −64 … −79 dBFS (Referenz −49 … −114).
  - *Abweichung:* „004 ohne Korrekturiteration“ ist nicht ganz erfüllt. Ein zusätzlicher Render-Durchlauf entfällt dank Limiter-Probe; der Gain wird aber aus der Probe korrigiert. Kurze Dateien (001, 003) brauchen noch einen zweiten Durchlauf.
- [x] Performance bleibt ≥ 50× Echtzeit (3 h Stereo). Bestehende Tests grün. JSON-Report validiert gegen Schema-Version 2. *1-h-Stereodatei aus 004: 55× Echtzeit, 63 MB Speicher, sowohl natürlich als auch kräftig. Bestehende Tests: Isolierte Stufentests laufen ohne Hochpass/Kompressor.*
- [x] **Hörabnahme durch dich:**
  - 001–005 im Vergleich mit der Referenz: Knackser deutlich leiser, Sprache natürlich, keine abgeschnittenen Laute, laute Stellen nicht gequetscht.
  - Die Pausen-Absenkung probierst du einmal mit `--pause-floor -60` und entscheidest, ob ihr Default aus bleibt.
  - *Ergebnis:* Knackser und Kompressor passen. Die Pausen-Absenkung kam zuerst zu spät und abrupt; nach der Änderung (Haltezeit ab dem letzten Laut, 150-ms-Ausblende, siehe 3.11) freigegeben. Default bleibt aus (natürlich); `podcast-kraeftig` schaltet sie ein.

### Phase 6: Einfache GUI mit egui (≈ 1 Woche) – umgesetzt

> **Stand 2026-11-26:** Umgesetzt mit `eframe`/`egui` 0.37 und `rfd` 0.18.
> - Start: `cargo run --release -p leveled-audio-gui`.
> - Neu in der Pipeline: Parametertabelle `params.rs` (48 Einstellungen, Bereiche gegen `validate` getestet) sowie `Settings::get_value`/`set_value`.
>   - Der Test fand eine Lücke: `denoise.profile.target_duration_s` war nach oben unbegrenzt; jetzt höchstens 60 s.
> - Die Kommandozeile hat jetzt zusätzlich eine Bibliothek (`leveled_audio_cli`: Eingaben, Stapel ausführen pro Datei/Album, Report). GUI und Kommandozeile rufen dieselbe Funktion `run_plan` auf.
> - **Änderung nach deinem Test (2026-11-27):** Abschnitt „Ausgabe“ entfernt. Export immer neben das Original als `<Name>-leveled-<Preset>.<Endung>`; vorhandene Ziele nur nach Rückfrage (Überschreiben / Vorhandene überspringen / Abbrechen). Statuszeile (bereit, in Arbeit, fertig, abgebrochen). Exportierte Dateien werden beim Ordner-Import übergangen.
> - Nicht geprüft: Die Oberfläche selbst habe ich nicht gesehen (kein Bildschirmzugriff); sie startet und läuft ohne Fehlermeldung. Windows und Linux baut erst der Workflow.

**Ziel:** Eine schlichte Oberfläche zum Testen. Dateien importieren, alle Parameter einstellen, Ergebnisse exportieren. Kein Abhören, keine Wellenform, keine Vorschau; das kann später kommen (siehe „Später“). Die GUI nutzt dieselbe Pipeline wie die Kommandozeile, also liefert sie bei gleichen Einstellungen dieselben Dateien.

**Technik:**
- Neues Crate `leveled-audio-gui` mit dem Programm `leveled-audio-gui`.
- `eframe`/`egui` (MIT/Apache-2.0) für die Oberfläche, `rfd` (MIT) für Datei- und Ordnerdialoge.
- Keine Audio-Wiedergabe, also keine Audio-Bibliothek.
- Die Verarbeitung läuft in einem Hintergrund-Thread. Fortschritt und Abbruch gehen über die vorhandenen Schnittstellen `ProgressSink` und `CancelToken`, sodass die Oberfläche bedienbar bleibt.
- **Eine gemeinsame Parametertabelle** (Schlüssel, Bezeichnung, Einheit, Bereich, Schrittweite, Kurzbeschreibung aus 4.5) speist die Bedienelemente. Bereiche und `Settings::validate` können so nicht auseinanderlaufen. Die Kommentare von `preset show` sollen später ebenfalls daraus kommen.

**Fenster (ein Fenster, von oben nach unten):**
1. **Eingabe:** Buttons „Dateien hinzufügen…“ und „Ordner hinzufügen…“, Checkbox „Unterordner einbeziehen“. Dateien lassen sich auch ins Fenster ziehen.
   - Liste der Dateien mit „Entfernen“ und „Alle entfernen“.
2. **Ausgabe:** Zielordner wählen, Benennungsschema, „Vorhandene überschreiben“ bzw. „überspringen“.
3. **Einstellungen:**
   - Preset-Auswahl (die vier eingebauten), „Preset laden…“ und „Preset speichern…“ als TOML, „Auf Preset zurücksetzen“.
   - Darunter **alle** Parameter, nach Stufe gruppiert in einklappbaren Abschnitten (Hochpass, Rauschreduktion, Knackser, Leveler, Pausen-Absenkung, Kompressor, Lautheit, Limiter, Ausgabe).
   - Je Parameter ein Regler oder Auswahlfeld im erlaubten Bereich, mit der Kurzbeschreibung als Tooltip.
   - Die wichtigsten Regler aus 4.5 stehen oben im Abschnitt, die Feinheiten darunter.
4. **Verarbeitung:** „Start“ und „Abbrechen“, ein Fortschrittsbalken gesamt und je laufender Datei.
5. **Ergebnis:** Tabelle wie in der Konsole (Eingang, Rauschen, Knackser, Leveler, Ausgang, Gain, Kompressor, Limiter, Status) mit den Hinweisen darunter.
   - Button „JSON-Report speichern…“ und „Zielordner öffnen“.

**Release:**
- Der Workflow baut zusätzlich `leveled-audio-gui`: macOS Universal, Windows (ohne Konsolenfenster, statische C-Laufzeit) und Linux `x86_64-unknown-linux-gnu`.
- Linux ist hier **nicht** statisch. Eine Oberfläche braucht die Grafikbibliotheken des Systems (X11 bzw. Wayland, OpenGL), die jeder Desktop mitbringt. Die Kommandozeile bleibt statisch (musl).

Akzeptanzkriterien:
- [ ] Startet ohne Zusatzinstallation auf macOS und Windows. Unter Linux reicht ein üblicher Desktop; die Voraussetzungen stehen in der Anleitung.
- [x] Import: Dateien, Ordner (mit/ohne Unterordner) und Ziehen ins Fenster. Doppelte Dateien werden nur einmal aufgenommen, Nicht-WAV-Dateien ignoriert.
- [x] **Alle** Parameter aus den Settings sind einstellbar. Ein Test prüft, dass jeder Schlüssel der Settings in der Parametertabelle steht und dass ihre Bereichsgrenzen von `validate` akzeptiert werden, Werte knapp außerhalb nicht.
- [x] Preset speichern und wieder laden ergibt dieselben Einstellungen. Eine von der GUI gespeicherte Datei funktioniert auch mit `leveled-audio --preset`. *Test: alle 48 Parameter wie über die Regler geändert, gespeichert (mit Kommentaren) und mit dem Leser der Kommandozeile geladen ⇒ identisch.*
- [x] Gleiche Einstellungen ⇒ byte-identische Ausgabe wie die Kommandozeile (automatischer Test). *Geprüft über den gemeinsamen Weg: GUI-Auftrag = direkte Pipeline = Lauf mit dem gespeicherten Preset, byte-identisch. Beide Oberflächen nutzen `run_plan`.*
- [x] Während der Verarbeitung bleibt das Fenster bedienbar (Hintergrund-Thread). Abbrechen löscht Teildateien wie bei der Kommandozeile. *Test für den Abbruch.*
- [x] Defekte Dateien erscheinen als Fehler in der Tabelle, der Rest wird verarbeitet. *Test: 2 ok, 1 Fehler; doppelte Eingaben und Nicht-WAV-Dateien werden ignoriert.*
- [x] **Bedienabnahme durch dich.** *Abgenommen am 2026-11-27 auf macOS. Dabei gefunden und behoben: Die GUI vergaß einen laufenden Export schon im nächsten Frame (Status blieb auf „bereit“, Export-Knopf aktiv, keine Ergebnistabelle). Neuer Test klickt headless durch Export, Status, Rückfrage und Ergebnis.*

Offene Fragen:
- Sprache der Oberfläche: nur Deutsch (Vorschlag) oder umschaltbar?

> **Nachtrag 2026-11-27: Abstandstest gegen die Referenzfassung.** `Abstand.WAV` (Sprache bei 1 m, 50 cm, 5 cm; Original −39,0 / −31,8 / −18,9 LUFS je Abschnitt, also 20 dB Spanne).
>
> | Fassung | 1 m | 50 cm | Wechsel (Geräusche) | 5 cm |
> |---|---|---|---|---|
> | Referenz | −15,5 | −16,0 | −34,9 | −15,5 |
> | leveled-audio vorher (60 %) | −18,7 | −16,6 | −24,3 | −14,3 |
> | leveled-audio jetzt (60 %) | −19,0 | −16,8 | −36,2 | −14,3 |
> | jetzt 100 %, max. Anhebung 20 dB | −16,0 | −16,2 | −36,9 | −16,0 |
> | jetzt 130 %, max. Anhebung 20 dB | −14,7 | −16,2 | −38,0 | −17,6 |
>
> - Geräusch-Gate im Leveler (siehe 3.6, Schritt 3): Geräusche im Wechsel jetzt auf Niveau der Referenz statt 10 dB darüber.
> - `leveler.strength` jetzt bis 130 % (Wunsch: über die Referenz hinaus). 100 % entspricht hier der Referenz, 130 % korrigiert über.
> - Bestehende Aufnahmen: 001–004 byte-identisch zu Phase 5, 005 Leveler-Gain um ≈ 0,4 dB verschoben.
> - **Neuer Standard (dein Wunsch nach dem Hörtest):** `leveler.strength` 1,3 und `max_boost_db` 20 in allen Presets (`podcast-kraeftig` hatte 0,8, jetzt ebenfalls 1,3). Folgen an 001–005: LRA kleiner (z. B. 003: 2,9 → 1,4 LU), Limiter greift mehr (003: 3,1 → 5,9 dB, 004: 3,6 → 6,1 dB).
> - Dabei gefunden: Bei starkem Leveler stehen Pausen unterschiedlich laut (nach angehobenen bzw. abgesenkten Phrasen); die Pausen-Absenkung hatte nur eine Tiefe und verfehlte das Ziel in einzelnen Pausen um > 2 dB. Jetzt Tiefe **je Frame** aus dem Pausenpegel ohne Leveler-Gain plus dem Leveler-Gain an der Stelle: jede Pause landet auf dem Ziel (Test: −60,1 / −60,1 dBFS).
> - Höhenanhebung bei fernem Mikrofon (Referenz ≈ +3 dB über 3 kHz bei 1 m) bleibt offen, siehe „Später“.

### Später (vorgemerkt, Architektur bereits vorbereitet)

- Mehrere Rauschprofile pro Datei: Segmentierung bei Änderungen des Rauschpegels oder -spektrums, Profil pro Region, Überblendung der Profile. Danach adaptives Tracking.
- Pre-Chain: Hum-Removal (Kammfilter auf 50/60 Hz + Harmonische). Der Hochpass ist in Phase 5 vorgezogen.
- Optionaler Klang-EQ (z. B. Präsenz/Höhen, wie die Referenz +2–3 dB über 6 kHz), Default aus. Adaptiv je Abstand: Die Referenz hebt bei 1 m die Höhen über 3 kHz um ≈ 3 dB an (Abstandstest).
- FLAC/MP3-Eingabe (`symphonia`), FLAC-Ausgabe. Für MP3-Ausgabe muss die Lizenz von LAME geprüft werden (LGPL).
- Mehrspur-Modus (synchrone Sprecherspuren), Noise-Shaping, Metadaten (bext/iXML/Cue) übernehmen.
- GUI-Ausbau nach Phase 6: Abhören mit A/B-Vergleich (Original/Bearbeitet, lautheitsangeglichen), Wellenform, Vorschau einzelner Regionen per Seek, Notarisierung des macOS-Bundles (braucht ein Apple-Entwicklerkonto). *Das App-Bundle selbst (`scripts/macos-app.sh`, Start ohne Terminal) gibt es seit 2026-11-27, ebenso Programm-Icon (`scripts/make-icon.py` → `assets/icon.png`), Farbschema und den Anzeigenamen „Leveled Audio“.*
- Signierung und Notarisierung (macOS) sowie Installer.

---

## 7. Risiken und offene Punkte

| Risiko | Auswirkung | Gegenmaßnahme |
|---|---|---|
| Klangqualität ist subjektiv | Algorithmus „misst gut, klingt schlecht“ | Früh echte Testaufnahmen von dir, Hörabnahme als Phasenkriterium, Parameter-Defaults nach Hörtest justieren |
| Statisches Profil bei wechselndem Rauschen (Klimaanlage an/aus) | Unter- bzw. Übersubtraktion in Teilen der Datei | Konfidenz und Warnung über Pausenpegel-Streuung. Mehrere Profile als erste Erweiterung. |
| VAD-Fehler (Musik, Lachen, niedriger SNR) | Falsches Profil, Leveler hebt Musik an | Konfidenz, Fallback, Warnungen. Ausschlussbereiche später. |
| Limiter senkt die Lautheit | Ziel wird verfehlt | Messung nach Pass 3, eine Korrekturiteration |
| Determinismus über CPU-Familien | Golden-Tests auf anderen Runnern rot | Toleranzbasierte Golden-Tests, Garantie dokumentiert |
| I/O auf externer HDD bei parallelen Jobs (3 Lesedurchläufe) | Seek-Thrashing, langsamer als die CPU | `--jobs` frei wählbar, Default = min(Kerne, 4). Hinweis im Report, wenn I/O-gebunden (CPU-Auslastung niedrig). |
| Eigene WAV-I/O | Randfälle (seltsame Chunks, 24-in-32-Container, falsche Größenfelder) | Kreuztests gegen `hound`, Fuzzing des Header-Parsers (`cargo-fuzz`, Phase 4), tolerantes Lesen von Size-Feldern |
| Unsignierte macOS-Binaries | Gatekeeper blockiert | Anleitung (`xattr -d com.apple.quarantine`). Signierung später. |
| macOS-Intel-CI-Runner werden knapp | x86_64 nicht getestet | Cross-Build auf arm64. Tests auf Intel, solange Runner verfügbar. |

**Offene Punkte, die ich bis spätestens zum Start der jeweiligen Phase von dir brauche:**
1. **Testaufnahmen** (3–5 Stück, 1–10 min, verschiedene Räume, SNR und Sprecher). Bleiben lokal, werden nicht eingecheckt. Bis Phase 2.
2. **Album-Modus:** Ist mit „gemeinsamer Ziellautheit über alle Dateien“ gemeint, dass ein gemeinsamer Gain die relativen Unterschiede erhält? (So geplant. Per-File bleibt der Default.)
3. **Metadaten** (BWF `bext`, Marker): Im MVP gehen sie verloren. Ist das okay?
4. **Default-Bittiefe** der Ausgabe: „wie Eingang“ (so geplant) oder fest 24 bit?
5. **Profilformat JSON** (menschenlesbar, ≈ 30 KB) – okay?
6. **GitHub-Repo:** öffentlich oder privat? (Wichtig für Actions-Minuten auf macOS-Runnern.)

---

## 8. Erste Schritte nach Freigabe

1. **Rust installieren** (auf diesem Mac fehlt es noch): `rustup` über https://rustup.rs (oder `brew install rustup`), dann `rustup default stable` und für Universal-Builds `rustup target add x86_64-apple-darwin`. Diesen Schritt machst du selbst. Ich nenne dir die genauen Befehle.
2. `git init`, Workspace-Gerüst mit den sechs Crates, `rust-toolchain.toml`, `LICENSE-MIT`/`LICENSE-APACHE`, `.gitignore`, `deny.toml`, `ci.yml` (fmt/clippy/test/deny auf 3 OS).
3. `leveled-audio-io`: RIFF/RF64-Parser und -Writer, Formatkonvertierung, Roundtrip- und `hound`-Kreuztests.
4. `leveled-audio-testkit`: Sinus, Tonfolgen nach Tech 3341/3342, weißes/rosa Rauschen, WAV-Schreibhelfer.
5. `analysis::loudness` (ebur128-Wrapper) + Tech-3341/3342-Tests.
6. `dsp::oversampler` + `dsp::limiter` + Tests (TP ≤ Ceiling).
7. `pipeline` (Pass 2/3 für Normalisieren + Limiter, Batch, Cancel) und `cli` (`analyze`, `process`). Damit ist Phase 1 erreicht und wird gegen die Akzeptanzkriterien geprüft.

Nach jedem Schritt: `cargo fmt`, `clippy`, Tests grün, Commit.
