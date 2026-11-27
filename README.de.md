# Leveled Audio

**Automatisches Leveling, Rauschreduktion und Lautheitsnormalisierung für Sprachaufnahmen. Gemacht für Podcasts, Videoproduktion, Interviews und Voice-over. Kostenlos, Open Source und offline.**

[![Release](https://img.shields.io/github/v/release/Marduk4all/leveled-audio)](https://github.com/Marduk4all/leveled-audio/releases/latest)
[![CI](https://github.com/Marduk4all/leveled-audio/actions/workflows/ci.yml/badge.svg)](https://github.com/Marduk4all/leveled-audio/actions/workflows/ci.yml)
[![Lizenz: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#lizenz)
![Plattformen](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)

[English](README.md) · **Deutsch**

Leveled Audio macht aus ungleichmäßigen Sprachaufnahmen in einem Durchgang fertigen, gleichmäßig lauten Ton:

- **Gleiche Sprecherpegel:** Leise und laute Sprecher werden angeglichen, auch wenn jemand zwischen 1 m und 5 cm vom Mikrofon wechselt.
- **Weniger Störgeräusche:** Raumrauschen wird reduziert, Griffgeräusche und Knackser werden entfernt.
- **Lautheit auf Ziel:** Das Ergebnis trifft ein Lautheitsziel in LUFS, z. B. −16 LUFS für Podcasts oder −23 LUFS für Rundfunk nach EBU R128, ohne Übersteuerung (True-Peak-Limiter).
- **Stapelbearbeitung:** ein ganzer Ordner mit WAV-Dateien auf einmal, auf deinem eigenen Rechner. Nichts wird hochgeladen, kein Konto, kein Abo.

Es gibt das Programm als **Desktop-App** (macOS, Windows, Linux) und als **Kommandozeilenwerkzeug** `leveled-audio` für Skripte und automatisierte Abläufe.

<p align="center">
  <img src="docs/images/gui-settings-de.png" width="420" alt="Leveled Audio: WAV-Dateien hinzufügen, Preset wählen, Leveler-Stärke einstellen">
  <img src="docs/images/gui-result-de.png" width="420" alt="Leveled Audio: Ergebnis des Exports mit Lautheit (LUFS) und True Peak je Datei">
</p>

<p align="center">
  <a href="https://github.com/Marduk4all/leveled-audio/releases/latest"><b>⬇ Neueste Version herunterladen</b></a> ·
  <a href="https://marduk4all.github.io/leveled-audio/de/">Webseite</a> ·
  <a href="docs/ANLEITUNG.md">Bedienungsanleitung</a> ·
  <a href="#schnellstart">Schnellstart</a>
</p>

---

## Anwendungsfälle

### 🎙 Podcasts
- **Interviews und Gesprächsrunden:** Gäste an verschiedenen Mikrofonen oder sehr unterschiedlich laut aufgenommene Remote-Gäste landen auf derselben Lautheit.
- **Pausen:** Pausen bleiben natürlich, oder werden mit dem Preset `podcast-kraeftig` fast still.
- **Lautheitsziel:** standardmäßig −16 LUFS, wie von den meisten Podcast-Plattformen empfohlen. Für Mono-Sendungen `podcast-mono` (−19 LUFS).
- **Ganze Staffeln:** Der Album-Modus gibt allen Folgen einen gemeinsamen Gain, damit sie untereinander gleich klingen.

### 🎬 Videoproduktion (YouTube, Social Media, Dokumentation, Firmenvideo)
- **Dialog und Voice-over:** Pegel vor der Endmischung angleichen, Interviewton von Ansteck- oder Kameramikrofonen säubern.
- **Wechselnder Abstand:** Wer beim Sprechen herumläuft oder sich vom Mikrofon wegdreht, bleibt gleichmäßig laut.
- **Abgabeziele:** −14 LUFS ist für YouTube und Streaming üblich, −23 LUFS (EBU R128) für Rundfunk. Das Ziel ist eine einzige Einstellung.
- **Ablauf:** Dialogspur als WAV aus dem Schnittprogramm exportieren (oder mit ffmpeg, siehe unten), im Stapel bearbeiten und wieder in die Timeline holen.

### 🎓 Vorlesungen, Kurse und Webinare
- **Unterschiedliche Sprecher:** Schwer verständliche und zu laute Sprecher werden angeglichen.
- **Raumaufnahmen:** Raumrauschen in Hörsaal-Mitschnitten und Bildschirmaufnahmen mit Kommentar wird reduziert.

### 📚 Hörbücher, Voice-over und Sprecheraufnahmen
- **Gleichmäßige Kapitel:** Viele Aufnahmesitzungen landen auf derselben Lautheit, mit kontrolliertem Spitzenpegel.

### 📻 Radio und Rundfunk
- **EBU R128:** Das Preset `ebu-r128` (−23 LUFS) erhält den natürlichen Raumton.
- **Archivierung:** Ein JSON-Report hält alle Messwerte für Dokumentation und Qualitätskontrolle fest.

---

## Funktionen

| | |
|---|---|
| **Adaptiver Leveler** | Gleicht unterschiedlich laute Sprecher und Phrasen an. Die Wechsel liegen in den Pausen, Pausen werden nicht angehoben. Die Stärke reicht bis 130 % (Überkorrektur) für große Abstandswechsel zum Mikrofon. |
| **Rauschreduktion** | Spektrale Rauschreduktion mit einem Rauschprofil, das automatisch in den ruhigsten Pausen jeder Datei gefunden oder einmal aus einer Raumton-Aufnahme gelernt wird. |
| **Griffgeräusche entfernen** | Erkennt tieffrequente Knackser und Griffgeräusche am Mikrofon, senkt sie ab und verhindert, dass der Leveler sie anhebt. |
| **Kompressor** | Fängt kurze laute Momente wie Lachen und Ausrufe sanft ab. |
| **Pausen-Absenkung** (optional) | Senkt Pausen auf einen Zielpegel ab, für einen dichten „Radio“-Klang. |
| **Lautheitsnormalisierung** | Integrierte Lautheit nach ITU-R BS.1770-4 / EBU R128, pro Datei oder als Album mit gemeinsamem Gain. |
| **True-Peak-Limiter** | 8-faches Oversampling, Standard −1 dBTP, sicher für MP3/AAC. |
| **Hochpass** | Entfernt Rumpeln und Trittschall. |
| **Presets** | `podcast-stereo`, `podcast-mono`, `podcast-kraeftig`, `ebu-r128`, dazu eigene TOML-Presets, die App und Kommandozeile gemeinsam nutzen. |
| **Reports** | Tabelle in der Konsole und JSON-Report (mit Schema) mit Lautheit, True Peak, LRA, Rauschpegel und Gain-Änderungen je Datei. |
| **Sicher** | Verändert nie die Originale, schreibt keine halbfertigen Dateien, liefert bei gleicher Eingabe byte-identische Ergebnisse und verarbeitet auch stundenlange Dateien mit nahezu konstantem Speicherbedarf. |
| **Formate** | WAV und RF64 als Ein- und Ausgabe: 16/24/32 bit int, 32 bit float, Mono und Stereo, übliche Abtastraten. |

**Verarbeitungskette:** Hochpass → Rauschreduktion + Knackser-Absenkung → Leveler (+ Pausen-Absenkung) → Kompressor → Lautheitsnormalisierung → True-Peak-Limiter.

### Lautheitsziele im Überblick

| Ziel | Lautheit | So geht's |
|---|---|---|
| Podcast (Stereo) | −16 LUFS | Preset `podcast-stereo` (Standard) |
| Podcast (Mono) | −19 LUFS | Preset `podcast-mono` |
| YouTube / Streaming-Video | −14 LUFS | `--target-lufs -14` oder die Einstellung „Ziel-Lautheit“ |
| Rundfunk (EBU R128) | −23 LUFS | Preset `ebu-r128` |
| Rundfunk (ATSC A/85, USA) | −24 LUFS | `--target-lufs -24` |

Die Empfehlungen der Plattformen ändern sich mit der Zeit, prüfe deshalb die aktuellen Vorgaben deiner Plattform.

---

## Schnellstart

### Desktop-App

1. **Herunterladen:** Lade das Archiv für dein System von der [neuesten Version](https://github.com/Marduk4all/leveled-audio/releases/latest) und entpacke es.
   - **macOS:** `Leveled Audio.app` in den Ordner „Programme“ ziehen. Beim ersten Start Rechtsklick → Öffnen, weil die App nicht notarisiert ist.
   - **Windows:** `leveled-audio-gui.exe` starten. Warnt SmartScreen, auf „Weitere Informationen“ → „Trotzdem ausführen“ klicken.
   - **Linux:** `leveled-audio-gui` starten.
2. **Hinzufügen:** WAV-Dateien oder ganze Ordner hinzufügen oder ins Fenster ziehen.
3. **Preset wählen** und bei Bedarf Einstellungen anpassen. Fährst du mit der Maus über einen Regler, erscheint eine Erklärung.
4. **Auf Exportieren klicken.** Jede Datei landet neben dem Original als `Name-leveled-<Preset>.wav`. Vor dem Überschreiben fragt die App nach.

Die App startet auf Englisch. Mit **EN | DE** oben rechts schaltest du auf Deutsch um.

### Kommandozeile

```bash
# Podcast: ganzen Ordner (rekursiv) auf −16 LUFS / −1 dBTP bringen
leveled-audio process aufnahmen/ -r -o fertig/

# Dialog für ein YouTube-Video auf −14 LUFS
leveled-audio process dialog.wav -o fertig/ --target-lufs -14

# Rundfunk: EBU R128 (−23 LUFS), 24 bit, mit JSON-Report
leveled-audio process interview.wav -o fertig/ --preset ebu-r128 --bit-depth 24 --report-json report.json

# Kräftigerer Podcast-Klang: Pausen fast still, dichter
leveled-audio process aufnahmen/ -o fertig/ --preset podcast-kraeftig

# Ganze Staffel mit gemeinsamem Gain (relative Unterschiede bleiben)
leveled-audio process staffel/ -o fertig/ --loudness-mode album

# Rauschprofil einmal aus Raumton lernen und für alle Dateien nutzen
leveled-audio profile learn raumton.wav -o studio.json
leveled-audio process aufnahmen/ -o fertig/ --noise-profile studio.json

# Nur messen: Lautheit, True Peak, LRA, Rauschpegel
leveled-audio analyze aufnahmen/*.wav
```

Alle Optionen erklärt die [Bedienungsanleitung](docs/ANLEITUNG.md#3-befehle).

### Video-Ablauf mit ffmpeg

Leveled Audio liest und schreibt WAV. Bei einer Videodatei holst du den Ton erst heraus, bearbeitest ihn und setzt ihn wieder ein:

```bash
# 1. Ton als 24-bit-WAV herauslösen
ffmpeg -i video.mp4 -vn -c:a pcm_s24le dialog.wav

# 2. Angleichen, entrauschen und auf −14 LUFS normalisieren
leveled-audio process dialog.wav -o fertig/ --target-lufs -14

# 3. Bearbeiteten Ton wieder einsetzen (Video wird kopiert, nicht neu kodiert)
ffmpeg -i video.mp4 -i fertig/dialog.wav -map 0:v -map 1:a -c:v copy -c:a aac -b:a 192k video-leveled.mp4
```

Bei geschnittenen Projekten exportierst du die Dialogspur aus dem Schnittprogramm (DaVinci Resolve, Premiere Pro, Final Cut Pro …) als WAV und holst das Ergebnis wieder hinein.

---

## Dokumentation

- **[Webseite](https://marduk4all.github.io/leveled-audio/de/):** Überblick, Anwendungsfälle und Download.
- **[Bedienungsanleitung (Deutsch)](docs/ANLEITUNG.md):** Installation, alle Einstellungen mit Empfehlungen, Presets, Album-Modus, JSON-Report, häufige Fragen.
- **[User guide (English)](docs/GUIDE.md)**
- **[Schema des JSON-Reports](docs/report.schema.json)**
- **[Entwicklungsplan](PLAN.md):** Designentscheidungen, Messungen, Ausblick.

## Selbst bauen

Du brauchst [Rust](https://rustup.rs) (die Version ist in `rust-toolchain.toml` festgelegt).

```bash
cargo build --release
scripts/macos-app.sh        # nur macOS: erzeugt target/release/Leveled Audio.app
```

Die Programme liegen danach in `target/release/` (`leveled-audio`, `leveled-audio-gui`).

Ein Release entsteht durch ein Tag (`git tag v0.2.0 && git push origin v0.2.0`). Der Workflow baut macOS (Universal), Windows und Linux, prüft die Programme und veröffentlicht sie mit `SHA256SUMS.txt`. Die Versionshinweise kommen aus `docs/releases/<tag>.md`.

## Entwicklung

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Screenshots, Programm-Icon und Vorschaubild neu erzeugen:

```bash
cargo run --release -p leveled-audio-gui --example screenshots -- /pfad/zum/arbeitsordner docs/images
scripts/make-icon.py 1024 assets/icon.png && scripts/make-icon.py 256 assets/icon-256.png
swift scripts/social-preview.swift          # nur macOS: docs/images/social-preview.png
```

Fehlerberichte und Ideen sind als [Issues](https://github.com/Marduk4all/leveled-audio/issues) willkommen, Fragen in den [Discussions](https://github.com/Marduk4all/leveled-audio/discussions). Siehe [CONTRIBUTING.md](CONTRIBUTING.md).

## Lizenz

Wahlweise unter [MIT](LICENSE-MIT) oder [Apache-2.0](LICENSE-APACHE). Die Desktop-App enthält Schriften unter eigenen Lizenzen, siehe [licenses/](licenses/).
