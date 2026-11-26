# leveled-audio – Bedienungsanleitung

leveled-audio bearbeitet Sprachaufnahmen (WAV) im Stapel, offline und ohne weitere Installation:

```
Rauschreduktion → Leveler → Loudness-Normalisierung → True-Peak-Limiter → Ausgabe
```

- **Rauschreduktion:** Das Rauschprofil wird automatisch aus den ruhigsten Sprechpausen jeder Datei bestimmt und spektral abgezogen.
- **Leveler:** Gleicht unterschiedlich laute Sprecher und Phrasen an. Die Gain-Wechsel liegen in den Pausen, Pausen werden nicht angehoben.
- **Normalisierung:** Bringt die integrierte Lautheit (ITU-R BS.1770-4 / EBU R128) auf das Ziel, pro Datei oder als Album.
- **True-Peak-Limiter:** Hält die Obergrenze mit 8-fachem Oversampling ein (Default −1 dBTP).

Eingabedateien werden nie verändert.

---

## 1. Installation

Lade das Archiv für dein System von der Release-Seite und entpacke es:

| System | Archiv |
|---|---|
| macOS (Apple Silicon und Intel) | `leveled-audio-<version>-universal-apple-darwin.tar.gz` |
| macOS nur Apple Silicon / nur Intel | `…-aarch64-apple-darwin` / `…-x86_64-apple-darwin` |
| Windows 10/11 (64 bit) | `leveled-audio-<version>-x86_64-pc-windows-msvc.zip` |
| Linux (64 bit, jede Distribution) | `leveled-audio-<version>-x86_64-unknown-linux-musl.tar.gz` |

Die Binaries haben keine Laufzeitabhängigkeiten: Windows braucht kein VC++-Redistributable, das Linux-Binary ist statisch gelinkt. Die Prüfsummen stehen in `SHA256SUMS.txt`:

```bash
shasum -a 256 -c SHA256SUMS.txt --ignore-missing      # macOS
sha256sum -c SHA256SUMS.txt --ignore-missing          # Linux
```

**macOS:** Die Binaries sind nicht signiert. Beim ersten Start blockiert Gatekeeper das Programm. Entferne dann die Quarantäne-Markierung:

```bash
xattr -d com.apple.quarantine ./leveled-audio
```

Lege `leveled-audio` (bzw. `leveled-audio.exe`) in einen Ordner im `PATH`, z. B. `/usr/local/bin` oder `~/bin`.

---

## 2. Schnellstart

```bash
# Ordner rekursiv bearbeiten: −16 LUFS, −1 dBTP, Rauschreduktion und Leveler an
leveled-audio process aufnahmen/ -r -o fertig/

# Nur messen, nichts schreiben
leveled-audio analyze aufnahmen/ -r
```

Nach der Verarbeitung erscheint eine Tabelle mit allen Dateien (Beispiel):

```
Datei     LUFS  dBTP │   dBFS Konf    NR │ Leveler │   LUFS  dBTP  LRA │  Gain  Lim  Status
          ─ Eingang ── │ ─── Rauschen ──── │         │ ──── Ausgang ──── │
004.wav  -25.4  -0.7 │  -35.2 0.73 -11.7 │   -2…+6 │  -16.0  -1.1  4.2 │  +9.6  4.6  ok
```

| Spalte | Bedeutung |
|---|---|
| Eingang LUFS / dBTP | integrierte Lautheit und True Peak der Eingabe |
| Rauschen dBFS | Pegel des Rauschprofils |
| Konf | Konfidenz des Profils, 0–1. **MS** heißt, dass keine ruhigen Pausen gefunden wurden und Minimum Statistics als Ersatz dient. **imp** heißt, das Profil wurde importiert. |
| NR | mittlere Absenkung in den Pausen (dB) |
| Leveler | Gain-Bereich des Levelers in Sprache (dB) |
| Ausgang | Lautheit, True Peak und Loudness Range der Ausgabe |
| Gain | Normalisierungs-Gain (dB) |
| Lim | größte Absenkung durch den Limiter (dB) |

Darunter stehen Hinweise, z. B. zu übersteuerten Eingaben oder zu einer Lautheitskorrektur.

---

## 3. Befehle

| Befehl | Zweck |
|---|---|
| `leveled-audio process <EINGABEN>… -o <ORDNER>` | Dateien verarbeiten |
| `leveled-audio analyze <EINGABEN>…` | nur messen: Lautheit, True Peak, LRA, Rauschpegel, Profil-Konfidenz, Leveler-Vorschau |
| `leveled-audio profile learn <WAV> -o <profil.json>` | Rauschprofil aus einer Aufnahme lernen, z. B. aus Raumton |
| `leveled-audio preset list` | eingebaute Presets auflisten |
| `leveled-audio preset show <name or datei.toml>` | Preset vollständig als TOML ausgeben |

Eingaben können einzelne WAV-Dateien und Ordner sein. Mit `-r` werden Unterordner einbezogen, und die Ordnerstruktur bleibt in der Ausgabe erhalten. Alle Befehle kennen `-q` (keine Ausgabe außer Fehlern) und `--help`.

### Wichtige Optionen für `process`

| Option | Wirkung |
|---|---|
| `--preset <name or datei.toml>` | `podcast-stereo` (Default, −16 LUFS), `podcast-mono` (−19), `ebu-r128` (−23) oder eigene Datei |
| `--target-lufs <LUFS>` | Ziel-Lautheit, z. B. `--target-lufs -18` |
| `--ceiling-dbtp <dBTP>` | Obergrenze des True Peak, z. B. `--ceiling-dbtp -2` |
| `--loudness-mode per-file\|album` | Lautheit pro Datei (Default) oder gemeinsamer Gain, siehe Abschnitt 5 |
| `--bit-depth same\|16\|24\|32f` | Bittiefe der Ausgabe (Default `same`; 32-bit-int-Eingaben werden zu 24 bit) |
| `--reduction-db <dB>` | maximale Rauschabsenkung (Default 12) |
| `--leveler-strength <0–100>` | Stärke des Levelers in Prozent (Default 60) |
| `--no-denoise`, `--no-leveler`, `--no-normalize`, `--no-limiter` | einzelne Stufen abschalten |
| `--set <SCHLÜSSEL=WERT>` | beliebige Einstellung überschreiben, mehrfach möglich, siehe Abschnitt 4 |
| `--naming "<muster>"` | Dateiname der Ausgabe; Platzhalter `{stem}`, `{ext}`, `{preset}`, `{lufs}` |
| `--overwrite` / `--skip-existing` | vorhandene Zieldateien ersetzen bzw. überspringen (ohne beides: Abbruch) |
| `-j <N>` | Anzahl paralleler Dateien (Default: Anzahl Kerne, höchstens 4) |
| `--report-json <datei>` | Report als JSON, siehe Abschnitt 6 |
| `--noise-profile <profil.json>` | dieses Rauschprofil für alle Dateien verwenden |
| `--save-profiles <ordner>` | automatisch bestimmte Profile als `<name>.noise.json` speichern |
| `--export-delta` | entfernte Anteile zusätzlich als `<name>.delta.wav` schreiben (zum Abhören) |

Beispiele:

```bash
# EBU R128, 24 bit, mit JSON-Report
leveled-audio process interview.wav -o fertig/ --preset ebu-r128 --bit-depth 24 --report-json report.json

# Stärker entrauschen und kontrollieren, was entfernt wurde
leveled-audio process interview.wav -o fertig/ --reduction-db 18 --export-delta

# Ein Profil aus Raumton für alle Dateien
leveled-audio profile learn raumton.wav -o studio.json
leveled-audio process aufnahmen/ -o fertig/ --noise-profile studio.json

# Dateinamen mit Ziel-Lautheit: interview_-16.wav
leveled-audio process interview.wav -o fertig/ --naming "{stem}_{lufs}"
```

---

## 4. Presets und Einstellungen

### Reihenfolge der Schichten

Spätere Schichten überschreiben frühere:

```
eingebautes Basis-Preset  ←  Preset-Datei  ←  --set  ←  eigene Flags (--target-lufs, --no-leveler …)
```

### Eigenes Preset

```bash
leveled-audio preset show podcast-stereo > mein.toml
```

Die Datei enthält alle Einstellungen. Du kannst alles löschen, was du nicht ändern willst. Fehlende Werte kommen vom Basis-Preset (`base`, Default `podcast-stereo`):

```toml
schema_version = 1
name = "funk"
base = "ebu-r128"        # −23 LUFS als Grundlage

[denoise]
reduction_db = 15.0

[leveler]
strength = 0.4
```

```bash
leveled-audio process aufnahmen/ -o fertig/ --preset mein.toml
```

Unbekannte Schlüssel sind ein Fehler. Ein Tippfehler wie `strenght` bricht also mit einer Meldung ab, statt still ignoriert zu werden.

### `--set`

Mit `--set` lässt sich jede Einstellung einzeln überschreiben, ohne eine Datei anzulegen:

```bash
leveled-audio process a.wav -o fertig/ --set denoise.reduction_db=15 --set loudness.mode=album
```

### Alle Einstellungen

| Schlüssel | Default | Bereich | Bedeutung |
|---|---|---|---|
| `denoise.enabled` | `true` | | Rauschreduktion an/aus |
| `denoise.gain_rule` | `wiener-dd` | `wiener-dd`, `subtract` | Gain-Regel; `wiener-dd` erzeugt weniger „Musical Noise“ |
| `denoise.reduction_db` | 12 | 0–40 | maximale Absenkung (dB) |
| `denoise.oversubtraction` | 1,3 | 0,5–4 | Überschätzung des Rauschens |
| `denoise.floor` | 0,05 | 0–1 | Spectral Floor (nur `subtract`) |
| `denoise.release_ms` | 40 | 0–1000 | Rückstellzeit des Gains |
| `denoise.freq_smooth_oct` | 0 | 0–1 | Glättung über die Frequenz (Oktaven) |
| `denoise.fft_size` | `auto` | `auto`, 512–8192 | FFT-Größe (`auto` ≈ 43 ms) |
| `denoise.stereo_link` | `false` | | gleicher Gain für beide Kanäle |
| `denoise.profile.min_duration_s` | 0,3 | 0,1–60 | Mindestmenge Pausenmaterial |
| `denoise.profile.target_duration_s` | 2 | ≥ min | Menge, ab der das Profil voll vertrauenswürdig ist |
| `denoise.profile.statistic` | `trimmed` | `mean`, `trimmed`, `median` | Mittelung über die Pausen |
| `denoise.profile.min_confidence` | 0,4 | 0–1 | darunter Minimum Statistics als Ersatz |
| `leveler.enabled` | `true` | | Leveler an/aus |
| `leveler.strength` | 0,6 | 0–1 | 0 = keine Wirkung, 1 = volle Angleichung |
| `leveler.max_boost_db` | 10 | 0–20 | größte Anhebung |
| `leveler.max_cut_db` | 10 | 0–20 | größte Absenkung |
| `leveler.pause_max_boost_db` | 0 | −10–10 | Obergrenze des Gains in langen Pausen |
| `leveler.slew_db_per_s` | 2 | 0,5–10 | größte Gain-Änderung innerhalb einer Phrase |
| `loudness.enabled` | `true` | | Normalisierung an/aus |
| `loudness.target_lufs` | −16 | −70 bis −5 | Ziel-Lautheit |
| `loudness.mode` | `per-file` | `per-file`, `album` | siehe Abschnitt 5 |
| `limiter.enabled` | `true` | | Limiter an/aus |
| `limiter.ceiling_dbtp` | −1 | −20–0 | Obergrenze True Peak |
| `limiter.lookahead_ms` | 5 | 0,5–50 | Vorausschau |
| `limiter.release_ms` | 100 | 1–2000 | Rückstellzeit |
| `limiter.oversampling` | 8 | 4, 8, 16 | Oversampling der Peak-Erkennung |
| `output.bit_depth` | `same` | `same`, `16`, `24`, `32f` | Bittiefe |
| `output.dither` | `auto` | `auto`, `on`, `off` | TPDF-Dither (`auto`: nur bei 16 bit) |

---

## 5. Lautheit: pro Datei oder als Album

**Pro Datei** (Default): Jede Datei trifft das Ziel für sich.

**Album** (`--loudness-mode album`): Alle Dateien eines Aufrufs bekommen **denselben** Gain. Ihre gemeinsame Lautheit trifft das Ziel. Unterschiede zwischen den Dateien bleiben erhalten, etwa ein leiseres Intro gegenüber dem Hauptteil. Dafür werden zuerst alle Dateien analysiert und erst danach gerendert. Vor der Tabelle steht eine Album-Zeile mit Gain und Album-Lautheit.

Dateien mit Fehlern und übersprungene Dateien (`--skip-existing`) zählen nicht zur Album-Lautheit.

**Korrektur bei starkem Limiting:** Greift der Limiter stark ein, liegt die Lautheit nach dem Rendern unter dem Ziel. Weicht sie um mehr als 0,5 LU ab, rendert leveled-audio die Datei bzw. im Album-Modus alle Dateien mit angepasstem Gain neu. Das passiert höchstens zweimal. Die Tabelle nennt die Korrektur unter „Hinweise“. Bei typischer Sprache mit −16 LUFS ist keine Korrektur nötig.

---

## 6. JSON-Report

`--report-json report.json` bei `process` bzw. `--json report.json` bei `analyze` schreibt alle Messwerte maschinenlesbar. Das Format ist in `report.schema.json` beschrieben (JSON Schema 2020-12).

- Die Dateien stehen in Eingabereihenfolge, unabhängig davon, welche zuerst fertig wurde.
- `settings` enthält die vollständigen effektiven Einstellungen. `settings_hash` ist eine Prüfsumme darüber: Zwei Reports mit gleicher Prüfsumme wurden gleich verarbeitet.
- `summary` zählt die Ergebnisse (ok, übersprungen, Fehler, abgebrochen).
- `album` erscheint nur im Album-Modus.
- Jede Datei hat einen `status`: `ok` mit `report`, `skipped`, `error` mit `message` oder `cancelled`. Werte, die nicht ermittelt wurden, sind `null`.

---

## 7. Sicherheit, Abbruch, Exit-Codes

- Eingabedateien werden nie überschrieben, auch nicht mit `--overwrite`.
- Ausgaben entstehen zuerst als `<name>.partial` und werden erst nach Erfolg umbenannt. Es bleiben also keine halben Dateien zurück.
- **Ctrl+C** beendet die laufenden Dateien sauber und löscht Teildateien. Ein zweites Ctrl+C beendet sofort.
- Eine defekte Eingabedatei bricht den Stapel nicht ab. Sie erscheint als FEHLER in der Tabelle und im Report.

| Exit-Code | Bedeutung |
|---|---|
| 0 | alles in Ordnung |
| 1 | mindestens eine Datei fehlgeschlagen (die anderen sind fertig) |
| 2 | Bedienfehler (falsche Option, unbekanntes Preset, Ziel existiert …) |
| 130 | mit Ctrl+C abgebrochen |

Gleiche Eingabe, gleiche Einstellungen und gleiches Binary ergeben byte-identische Ausgaben. Zwischen verschiedenen CPU-Architekturen, etwa Apple Silicon und Intel, weichen einzelne Samples um wenige LSB ab (Differenz unter −120 dBFS).

---

## 8. Formate und Grenzen

- **Eingabe:** WAV und RF64, PCM 16/24/32 bit int und 32 bit float, Mono und Stereo, übliche Abtastraten (getestet mit 16–96 kHz).
- **Ausgabe:** WAV mit 16/24 bit oder 32 bit float. Ab 4 GiB wird automatisch RF64 geschrieben.
- **Lange Dateien:** Der Speicherbedarf ist nahezu konstant, eine 3-Stunden-Stereodatei braucht etwa 100 MB. Im Album-Modus bleiben bis zum Rendern je Datei rund 10 MB pro Stunde Audio im Speicher.
- **Geschwindigkeit:** Auf einem aktuellen Rechner etwa 60–70× Echtzeit pro Datei mit allen Stufen. Mehrere Dateien laufen parallel.
- **Nicht reparierbar:** Bereits übersteuerte (geclippte) Eingaben. Sie werden erkannt und gemeldet.
- **Bekannte Eigenheiten:**
  - Musik und Jingles werden mitgelevelt.
  - Ein schwankender Mikrofonabstand innerhalb einer Phrase wird nur langsam ausgeglichen. Das ist ein bewusster Kompromiss gegen „Pumpen“.

---

## 9. Häufige Fragen

**Die Konfidenz ist niedrig oder „MS“ steht in der Tabelle.**
In der Aufnahme gibt es kaum ruhige Pausen. Hilfreich ist eine kurze Raumton-Aufnahme: Lerne daraus ein Profil mit `profile learn` und verwende es mit `--noise-profile`.

**Die Rauschreduktion klingt blubbernd oder metallisch.**
Senke `--reduction-db` (z. B. auf 8). Höre mit `--export-delta`, was entfernt wird. In der Delta-Datei sollte keine Sprache zu hören sein.

**Der Leveler hebt zu viel bzw. zu wenig an.**
Ändere `--leveler-strength` (0–100) oder begrenze mit `--set leveler.max_boost_db=6`.

**Die Ausgabe ist leiser als das Ziel.**
Steht unter „Hinweise“, dass das Ziel verfehlt wurde, greift der Limiter sehr stark ein. Das betrifft vor allem schon stark komprimiertes Material oder große Anhebungen. Hilfreich ist ein leiseres Ziel oder ein höheres Ceiling, z. B. `--ceiling-dbtp -0.5`.
