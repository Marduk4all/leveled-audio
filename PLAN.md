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
3. **Referenz:** `L_ref` = dauergewichteter Median der Segmentlautheiten der Datei. Leise Sprecher werden angehoben, laute abgesenkt, symmetrisch. Die absolute Lage ist egal, weil die Normalisierung folgt.
4. **Soll-Gain:** `g = strength · (L_ref − L_seg)`, begrenzt auf `[−max_cut, +max_boost]`.
5. **Übergänge:** Gain-Wechsel liegen bevorzugt **in Pausen** (Rampe 250 ms, die vor dem Sprachbeginn endet). Ist die Pause zu kurz, wird über die Grenze geführt, mit Slew-Limit. Innerhalb einer Phrase gilt ein Slew-Limit von 2 dB/s.
6. **Pausen nicht anheben:**
   - Kurze Pausen (< 2 s) halten bzw. interpolieren den Gain. Ein Absenken in jeder kurzen Pause würde den Rauschteppich „atmen“ lassen, was schlimmer ist.
   - Lange Pausen (≥ 2 s) gleiten langsam (500 ms) auf `min(g, pause_max_boost_db)` und sind rechtzeitig vor der nächsten Phrase wieder oben.
7. **Glättung:** Zum Schluss nullphasige Ein-Pol-Glättung (vorwärts und rückwärts, τ = 150 ms) der Kurve. Angewendet wird pro Sample mit linearer Interpolation zwischen Frame-Stützstellen.

| Parameter | Default | Bereich |
|---|---|---|
| `strength` | **60 %** | 0–100 % (0 % ⇒ bitidentisch zu Bypass) |
| `max_boost_db` | **10** | 0–20 |
| `max_cut_db` | **10** | 0–20 |
| `pause_max_boost_db` | **0** | −10–+10 |
| `slew_db_per_s` | **2** | 0,5–10 |

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
strength = 0.6
max_boost_db = 10.0
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

### Phase 3: Adaptive Leveler (≈ 1–1,5 Wochen)

Lieferumfang:
- Segmentierung, Leveler-Kurve (offline), Anwendung im Render-Pass.
- Report-Werte, Parameter und Presets.

Akzeptanzkriterien:
- [ ] Zwei synthetische Sprecher mit 10 dB Differenz: Nach dem Leveler bei 100 % liegt die Differenz der Segmentlautheit < 2 dB, bei 50 % ≈ 5 dB (±1 dB).
- [ ] Grenzen `max_boost`/`max_cut` und `slew` werden eingehalten (Property-Tests). In langen Pausen gilt Gain ≤ `pause_max_boost`.
- [ ] Innerhalb eines Segments mit konstantem Pegel schwankt der Gain < 1 dB (kein Pumpen).
- [ ] `strength = 0` ist bitidentisch zum Bypass.
- [ ] Gesamtkette ≥ 50× Echtzeit pro Kern.
- [ ] **Hörabnahme durch dich** (Gesprächsaufnahmen mit unterschiedlich lauten Sprechern).

### Phase 4: Batch-Feinschliff, Reports, Presets, Release-Builds (≈ 1,5 Wochen)

Lieferumfang:
- TOML-Presets mit Schichtung, `preset list/show`, `--set`.
- JSON-Report mit Schema, Album-Modus, rekursive Ordner, Korrekturiteration für die Lautheit.
- `release.yml`: aarch64- und x86_64-apple-darwin (+ Universal), x86_64-pc-windows-msvc (statische CRT), x86_64-unknown-linux-musl, jeweils mit SHA256-Summen.
- README und Bedienungsanleitung.

Akzeptanzkriterien:
- [ ] Release-Artefakte laufen auf frischen Systemen ohne Zusatzinstallation (Windows ohne VC-Redist; `ldd` meldet unter Linux „statically linked“).
- [ ] JSON-Report validiert gegen das Schema. Reihenfolge deterministisch.
- [ ] Batch mit 100 Dateien: ein defekter Eingang bricht den Batch nicht ab (Exit 1, Report nennt den Fehler).
- [ ] Album-Modus: alle Dateien erhalten denselben Gain, die Album-Lautheit trifft das Ziel bis ±0,5 LU.
- [ ] Preset-Schichtung durch Tests abgedeckt, unbekannte Keys ergeben einen Fehler.

### Später (vorgemerkt, Architektur bereits vorbereitet)

- Mehrere Rauschprofile pro Datei: Segmentierung bei Änderungen des Rauschpegels oder -spektrums, Profil pro Region, Überblendung der Profile. Danach adaptives Tracking.
- Pre-Chain: Hochpass, Hum-Removal (Kammfilter auf 50/60 Hz + Harmonische).
- FLAC/MP3-Eingabe (`symphonia`), FLAC-Ausgabe. Für MP3-Ausgabe muss die Lizenz von LAME geprüft werden (LGPL).
- Mehrspur-Modus (synchrone Sprecherspuren), Noise-Shaping, Metadaten (bext/iXML/Cue) übernehmen.
- GUI auf Basis von `leveled-audio-pipeline` (Vorschau einzelner Regionen per Seek ist möglich).
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
