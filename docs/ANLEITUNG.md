# leveled-audio – Bedienungsanleitung

leveled-audio bearbeitet Sprachaufnahmen (WAV) im Stapel, offline und ohne weitere Installation:

```
Hochpass → Rauschreduktion + Knackser-Absenkung → Leveler (+ Pausen-Absenkung)
         → Kompressor → Loudness-Normalisierung → True-Peak-Limiter → Ausgabe
```

- **Hochpass:** Entfernt Rumpeln und Trittschall unter etwa 70 Hz.
- **Rauschreduktion:** Das Rauschprofil wird automatisch aus den ruhigsten Sprechpausen jeder Datei bestimmt und spektral abgezogen.
- **Knackser-Absenkung:** Erkennt Griffgeräusche und Knackser (tieffrequente Störungen ohne Sprachanteil) und senkt dort die Tiefen ab. Der Leveler hebt sie nicht an.
- **Leveler:** Gleicht unterschiedlich laute Sprecher und Phrasen an. Die Gain-Wechsel liegen in den Pausen, Pausen werden nicht angehoben.
- **Pausen-Absenkung** (optional, Default aus): Senkt Sprechpausen auf einen Zielpegel ab.
- **Kompressor:** Fängt kurze laute Stellen (Lachen, Ausrufe) sanft ab, bevor der Limiter hart begrenzen muss.
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

**Grafische Oberfläche:** Neben der Kommandozeile liegt `leveled-audio-gui` (Windows: `leveled-audio-gui.exe`) im Archiv. Unter Linux gibt es sie als eigenes Archiv `…-x86_64-unknown-linux-gnu`. Sie braucht die Grafik- und GTK-Bibliotheken eines üblichen Desktops (X11 oder Wayland, GTK 3), die jede Desktop-Distribution mitbringt. Siehe Abschnitt 3a.

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
Datei     LUFS  dBTP │   dBFS Konf    NR  Kn │ Leveler │   LUFS  dBTP  LRA │  Gain  Kmp  Lim  Status
          ─ Eingang ── │ ───── Rauschen ────── │         │ ──── Ausgang ──── │
004.wav  -25.4  -0.7 │  -54.2 0.83 -11.3  21 │   -3…+7 │  -16.3  -1.1  3.3 │ +10.6  0.4  3.6  ok
```

| Spalte | Bedeutung |
|---|---|
| Eingang LUFS / dBTP | integrierte Lautheit und True Peak der Eingabe |
| Rauschen dBFS | Pegel des Rauschprofils |
| Konf | Konfidenz des Profils, 0–1. **MS** heißt, dass keine ruhigen Pausen gefunden wurden und Minimum Statistics als Ersatz dient. **imp** heißt, das Profil wurde importiert. |
| NR | mittlere Absenkung in den Pausen (dB) |
| Kn | Anzahl erkannter Knackser bzw. Griffgeräusche |
| Leveler | Gain-Bereich des Levelers in Sprache (dB) |
| Ausgang | Lautheit, True Peak und Loudness Range der Ausgabe |
| Gain | Normalisierungs-Gain (dB) |
| Kmp / Lim | größte Absenkung durch Kompressor bzw. Limiter (dB) |

Darunter stehen Hinweise, z. B. zu übersteuerten Eingaben, zur Pausen-Absenkung oder zu einer Lautheitskorrektur.

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
| `--preset <name or datei.toml>` | `podcast-stereo` (Default, natürlich, −16 LUFS), `podcast-kraeftig` (Pausen fast still, dichter), `podcast-mono` (−19), `ebu-r128` (−23) oder eigene Datei |
| `--target-lufs <LUFS>` | Ziel-Lautheit, z. B. `--target-lufs -18` |
| `--ceiling-dbtp <dBTP>` | Obergrenze des True Peak, z. B. `--ceiling-dbtp -2` |
| `--loudness-mode per-file\|album` | Lautheit pro Datei (Default) oder gemeinsamer Gain, siehe Abschnitt 6 |
| `--bit-depth same\|16\|24\|32f` | Bittiefe der Ausgabe (Default `same`; 32-bit-int-Eingaben werden zu 24 bit) |
| `--reduction-db <dB>` | maximale Rauschabsenkung (Default 12) |
| `--leveler-strength <0–130>` | Stärke des Levelers in Prozent (Default 130, über 100 überkorrigiert) |
| `--highpass <Hz>` | Hochpass-Frequenz (Default 70) |
| `--knock-reduction <dB>` | Absenkung der Tiefen in Knacksern (Default 15; 0 = nur erkennen) |
| `--compression <0–100>` | Kompressor in Prozent (Default 40) |
| `--pause-floor <dBFS>` | Pausen-Absenkung einschalten, Zielpegel der Pausen, z. B. `-60` |
| `--no-denoise`, `--no-leveler`, `--no-normalize`, `--no-limiter`, `--no-highpass`, `--no-knock`, `--no-compression`, `--no-pause-floor` | einzelne Stufen abschalten |
| `--set <SCHLÜSSEL=WERT>` | beliebige Einstellung überschreiben, mehrfach möglich, siehe Abschnitt 5 |
| `--naming "<muster>"` | Dateiname der Ausgabe; Platzhalter `{stem}`, `{ext}`, `{preset}`, `{lufs}` |
| `--overwrite` / `--skip-existing` | vorhandene Zieldateien ersetzen bzw. überspringen (ohne beides: Abbruch) |
| `-j <N>` | Anzahl paralleler Dateien (Default: Anzahl Kerne, höchstens 4) |
| `--report-json <datei>` | Report als JSON, siehe Abschnitt 7 |
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

## 3a. Grafische Oberfläche „Leveled Audio“ (`leveled-audio-gui`)

**Starten ohne Terminal:**
- **macOS:** Im Archiv liegt `Leveled Audio.app`. Ziehe es in den Ordner „Programme“ und starte es per Doppelklick, aus dem Launchpad oder dem Dock. Beim ersten Start meldet macOS, dass die App von einem nicht verifizierten Entwickler stammt (sie ist nicht bei Apple notarisiert). Dann einmal mit der rechten Maustaste auf die App klicken → „Öffnen“ → „Öffnen“, oder unter Systemeinstellungen → Datenschutz & Sicherheit → „Dennoch öffnen“. Selbst gebaut: `cargo build --release -p leveled-audio-gui` und danach `scripts/macos-app.sh`; das Bundle liegt dann in `target/release/Leveled Audio.app`.
- **Windows:** `leveled-audio-gui.exe` per Doppelklick; es öffnet sich kein Konsolenfenster.
- **Linux:** `leveled-audio-gui` aus dem Dateimanager starten oder einen Starter (`.desktop`-Datei) anlegen.

Eine schlichte Oberfläche zum Testen: Dateien importieren, alle Einstellungen vornehmen, Ergebnisse exportieren. Sie nutzt dieselbe Verarbeitung wie die Kommandozeile. Bei gleichen Einstellungen entstehen dieselben Dateien.

**Sprache:** Die Oberfläche startet auf Englisch. Oben rechts schaltet `EN` / `DE` jederzeit zwischen Englisch und Deutsch um. Warnungen und Fehlermeldungen aus der Verarbeitung erscheinen derzeit immer auf Deutsch.

Das Fenster ist von oben nach unten aufgebaut:

1. **Eingabe:** „Dateien hinzufügen…“ und „Ordner hinzufügen…“, oder Dateien und Ordner ins Fenster ziehen. „Unterordner einbeziehen“ entspricht `-r`. Darunter steht, wie viele WAV-Dateien gefunden wurden. Bereits exportierte Dateien (`-leveled-` im Namen) werden beim Ordner-Import übergangen; einzeln hinzugefügt werden sie verarbeitet.
2. **Einstellungen:**
   - Preset wählen, ein eigenes Preset laden oder die aktuellen Werte als Preset speichern. Die gespeicherte Datei funktioniert auch mit `leveled-audio --preset`.
   - „Zurücksetzen“ stellt das gewählte bzw. geladene Preset wieder her.
   - Darunter alle Einstellungen nach Stufe gruppiert. Jede Gruppe zeigt oben die wichtigen Regler, „Feineinstellungen“ den Rest. Fährst du mit der Maus über einen Regler, erscheint die Kurzbeschreibung aus Abschnitt 4.
3. **Exportieren / Abbrechen** am unteren Fensterrand, daneben die Anzahl paralleler Dateien und der Status (bereit, in Arbeit, fertig, abgebrochen) mit Fortschritt gesamt und je Datei. Abbrechen löscht Teildateien.
   - Jede Datei wird **neben dem Original** abgelegt: `aufnahme.wav` → `aufnahme-leveled-podcast-stereo.wav` (bei einem geladenen Preset dessen Dateiname).
   - Gibt es Zieldateien schon, fragt ein Dialog: „Überschreiben“, „Vorhandene überspringen“ oder „Abbrechen“. Originale werden nie verändert.
4. **Ergebnis:** Tabelle wie in der Konsole, Hinweise, „JSON-Report speichern…“ und „Ordner öffnen“.

Abhören, Wellenform und Vorschau gibt es in dieser ersten Version nicht.

---

## 4. Regler: Empfehlungen und Wirkung

**Fix oder per Analyse?** Die Werte der Regler sind fest. Sie stehen im Preset, gelten für alle Dateien gleich und machen das Ergebnis reproduzierbar. Fast alle Regler beschreiben aber eine Absicht relativ zur jeweiligen Datei:
- Die Rauschreduktion arbeitet mit dem automatisch gefundenen Rauschprofil.
- Der Leveler gleicht auf den Sprachpegel der Datei an.
- Die Kompressor-Schwelle liegt 8 dB über diesem Sprachpegel.
- Die Pausen-Absenkung rechnet ihre Tiefe aus, damit die Pausen ihr Ziel erreichen.
- Die Knackser-Erkennung bewertet jeden Abschnitt einzeln.

Was tatsächlich angewendet wurde, steht im Report. Absolut gemeint sind nur Ziel-Lautheit, Spitzen-Grenze und Hochpass-Frequenz.

| Regler | Natürlich (`podcast-stereo`, Default) | Kräftig (`podcast-kraeftig`) | Was sich ändert |
|---|---|---|---|
| `--target-lufs` | −16 | −16 | Gesamtlautheit. Plattformen normalisieren meist auf −14 bis −16. |
| `--ceiling-dbtp` | −1 | −1 | Obergrenze für Spitzen. Höher entlastet den Limiter, riskiert aber Verzerrungen nach MP3/AAC. |
| `--highpass` | 70 Hz | 80 Hz | Entfernt Rumpeln. Ab etwa 100 Hz verlieren tiefe Stimmen Wärme. |
| `--knock-reduction` | 15 dB | 20 dB | Knackser leiser. Gleichzeitige Sprache klingt dort kurz dünner. |
| `--reduction-db` | 12 dB | 15 dB | Weniger Rauschen. Ab etwa 15–18 dB drohen „blubbernde“ Artefakte. |
| `--leveler-strength` | 130 % | 130 % | Gleicht auch starke Abstandswechsel (1 m ↔ 5 cm) aus; leise Stellen landen etwas über den lauten. 100 % = gleich laut, 60 % natürlicher und lebendiger, aber Pegelunterschiede bleiben hörbar. |
| `--compression` | 40 % (1,5:1) | 60 % (2:1) | Laute Stellen dichter. Zu hoch klingt gepresst. |
| `--pause-floor` | aus | −60 dBFS | Pausen fast still. Kann an Pausengrenzen „atmen“. |
| `--loudness-mode` | per-file | per-file | `album` nur für Folgen einer Produktion. |

`leveled-audio preset show <name>` gibt ein Preset mit diesen Kurzbeschreibungen als Kommentaren aus. `leveled-audio process --help` beschreibt jeden Regler.

---

## 5. Presets und Einstellungen

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
| `highpass.enabled` | `true` | | Hochpass an/aus |
| `highpass.frequency_hz` | 70 | 20–200 | Grenzfrequenz (Hz) |
| `denoise.enabled` | `true` | | Rauschreduktion an/aus (ohne sie auch keine Knackser-Absenkung) |
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
| `knock.enabled` | `true` | | Knackser/Griffgeräusche erkennen |
| `knock.max_speech_to_low_db` | −15 | −30 … −5 | Sprachband (300–3400 Hz) so weit unter den Tiefen (20–300 Hz) ⇒ Störgeräusch |
| `knock.reduction_db` | 15 | 0–40 | Absenkung der Tiefen in Knacksern; 0 = nur erkennen |
| `leveler.enabled` | `true` | | Leveler an/aus |
| `leveler.strength` | 1,3 | 0–1,3 | 0 = keine Wirkung, 1 = volle Angleichung, darüber Überkorrektur |
| `leveler.max_boost_db` | 20 | 0–20 | größte Anhebung |
| `leveler.max_cut_db` | 10 | 0–20 | größte Absenkung |
| `leveler.pause_max_boost_db` | 0 | −10–10 | Obergrenze des Gains in langen Pausen |
| `leveler.slew_db_per_s` | 2 | 0,5–10 | größte Gain-Änderung innerhalb einer Phrase |
| `gate.enabled` | `false` | | Pausen-Absenkung an/aus |
| `gate.floor_dbfs` | −60 | −90 … −30 | Zielpegel der Pausen in der fertigen Datei |
| `gate.max_attenuation_db` | 30 | 0–60 | höchstens so viel zusätzliche Absenkung |
| `gate.min_pause_ms` | 400 | 100–2000 | kürzere Pausen bleiben unverändert |
| `gate.hold_ms` | 50 | 0–500 | Halten nach dem letzten Laut, bevor ausgeblendet wird |
| `gate.fade_out_ms` | 150 | 10–1000 | Ausblenden in die Pause; länger = sanfter, aber später leise |
| `gate.fade_in_ms` | 50 | 10–500 | Aufblenden vor dem nächsten Wort |
| `gate.pre_open_ms` | 50 | 0–200 | so viel früher vor Sprachbeginn wieder ganz offen |
| `compressor.enabled` | `true` | | Kompressor an/aus |
| `compressor.amount` | 0,4 | 0–1 | 0,4 = 1,5:1, 0,6 = 2:1, 1 = 6:1 |
| `compressor.threshold_above_ref_db` | 8 | 0–20 | Schwelle über dem Sprachpegel der Datei |
| `compressor.attack_ms` | 5 | 0,5–50 | Ansprechzeit |
| `compressor.release_ms` | 150 | 20–1000 | Rückstellzeit |
| `loudness.enabled` | `true` | | Normalisierung an/aus |
| `loudness.target_lufs` | −16 | −70 bis −5 | Ziel-Lautheit |
| `loudness.mode` | `per-file` | `per-file`, `album` | siehe Abschnitt 6 |
| `limiter.enabled` | `true` | | Limiter an/aus |
| `limiter.ceiling_dbtp` | −1 | −20–0 | Obergrenze True Peak |
| `limiter.lookahead_ms` | 5 | 0,5–50 | Vorausschau |
| `limiter.release_ms` | 100 | 1–2000 | Rückstellzeit |
| `limiter.oversampling` | 8 | 4, 8, 16 | Oversampling der Peak-Erkennung |
| `output.bit_depth` | `same` | `same`, `16`, `24`, `32f` | Bittiefe |
| `output.dither` | `auto` | `auto`, `on`, `off` | TPDF-Dither (`auto`: nur bei 16 bit) |

---

## 6. Lautheit: pro Datei oder als Album

**Pro Datei** (Default): Jede Datei trifft das Ziel für sich.

**Album** (`--loudness-mode album`): Alle Dateien eines Aufrufs bekommen **denselben** Gain. Ihre gemeinsame Lautheit trifft das Ziel. Unterschiede zwischen den Dateien bleiben erhalten, etwa ein leiseres Intro gegenüber dem Hauptteil. Dafür werden zuerst alle Dateien analysiert und erst danach gerendert. Vor der Tabelle steht eine Album-Zeile mit Gain und Album-Lautheit.

Dateien mit Fehlern und übersprungene Dateien (`--skip-existing`) zählen nicht zur Album-Lautheit.

**Korrektur bei starkem Limiting:** Greift der Limiter ein, liegt die Lautheit etwas unter dem Ziel. leveled-audio schätzt diesen Verlust schon beim Messen und gleicht ihn im Gain aus. Weicht die fertige Datei trotzdem um mehr als 0,3 LU ab, rendert leveled-audio sie mit angepasstem Gain neu, im Album-Modus alle Dateien. Das passiert höchstens zweimal. Größere Korrekturen (ab 0,5 dB) nennt die Tabelle unter „Hinweise“.

---

## 7. JSON-Report

`--report-json report.json` bei `process` bzw. `--json report.json` bei `analyze` schreibt alle Messwerte maschinenlesbar. Das Format ist in `report.schema.json` beschrieben (JSON Schema 2020-12).

- Die Dateien stehen in Eingabereihenfolge, unabhängig davon, welche zuerst fertig wurde.
- `settings` enthält die vollständigen effektiven Einstellungen. `settings_hash` ist eine Prüfsumme darüber: Zwei Reports mit gleicher Prüfsumme wurden gleich verarbeitet.
- `summary` zählt die Ergebnisse (ok, übersprungen, Fehler, abgebrochen).
- `album` erscheint nur im Album-Modus.
- Jede Datei hat einen `status`: `ok` mit `report`, `skipped`, `error` mit `message` oder `cancelled`. Werte, die nicht ermittelt wurden, sind `null`.

---

## 8. Sicherheit, Abbruch, Exit-Codes

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

## 9. Formate und Grenzen

- **Eingabe:** WAV und RF64, PCM 16/24/32 bit int und 32 bit float, Mono und Stereo, übliche Abtastraten (getestet mit 16–96 kHz).
- **Ausgabe:** WAV mit 16/24 bit oder 32 bit float. Ab 4 GiB wird automatisch RF64 geschrieben.
- **Lange Dateien:** Der Speicherbedarf ist nahezu konstant, eine 3-Stunden-Stereodatei braucht etwa 100 MB. Im Album-Modus bleiben bis zum Rendern je Datei rund 10 MB pro Stunde Audio im Speicher.
- **Geschwindigkeit:** Auf einem aktuellen Rechner etwa 55× Echtzeit pro Datei mit allen Stufen (gemessen an einer 1-Stunden-Stereodatei). Sehr kurze Dateien mit nachträglicher Lautheitskorrektur sind langsamer. Mehrere Dateien laufen parallel.
- **Nicht reparierbar:** Bereits übersteuerte (geclippte) Eingaben. Sie werden erkannt und gemeldet.
- **Bekannte Eigenheiten:**
  - Musik und Jingles werden mitgelevelt.
  - Ein schwankender Mikrofonabstand innerhalb einer Phrase wird nur langsam ausgeglichen. Das ist ein bewusster Kompromiss gegen „Pumpen“.

---

## 10. Häufige Fragen

**Die Konfidenz ist niedrig oder „MS“ steht in der Tabelle.**
In der Aufnahme gibt es kaum ruhige Pausen. Hilfreich ist eine kurze Raumton-Aufnahme: Lerne daraus ein Profil mit `profile learn` und verwende es mit `--noise-profile`.

**Knackser oder Griffgeräusche sind noch zu hören.**
Erhöhe `--knock-reduction` (z. B. 25). Wird ein Knackser nicht erkannt, weil er auch viel Energie im Sprachband hat, hilft ein höherer Wert für `--set knock.max_speech_to_low_db=-12`. Dann steigt aber das Risiko, dass sehr tiefe Sprachlaute mit abgesenkt werden.

**Die Pausen sollen leiser sein.**
Nutze `--pause-floor -60` oder das Preset `podcast-kraeftig`. Hört man das Rauschen an den Pausengrenzen kommen und gehen, wähle ein höheres Ziel (z. B. −55) oder senke die Rauschreduktion zusätzlich mit `--reduction-db 15`.

**Die Rauschreduktion klingt blubbernd oder metallisch.**
Senke `--reduction-db` (z. B. auf 8). Höre mit `--export-delta`, was entfernt wird. In der Delta-Datei sollte keine Sprache zu hören sein.

**Der Leveler hebt zu viel bzw. zu wenig an.**
Ändere `--leveler-strength` (0–130) oder begrenze mit `--set leveler.max_boost_db=6`. Die Anhebung ist bei `leveler.max_boost_db` (Default 20 dB) gedeckelt.

Kurze, rauschartige Geräusche zwischen Phrasen (Atmen, Rascheln, Umgreifen des Mikrofons) hebt der Leveler nicht an, auch wenn die Spracherkennung sie als Sprache einstuft.

**Die Ausgabe ist leiser als das Ziel.**
Steht unter „Hinweise“, dass das Ziel verfehlt wurde, greift der Limiter sehr stark ein. Das betrifft vor allem schon stark komprimiertes Material oder große Anhebungen. Hilfreich ist ein leiseres Ziel oder ein höheres Ceiling, z. B. `--ceiling-dbtp -0.5`.
