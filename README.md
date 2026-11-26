# leveled-audio (Arbeitstitel)

Batch-Verarbeitung von Sprachaufnahmen (WAV) per Kommandozeile – offline, ohne Laufzeitabhängigkeiten.

**Stand: Phase 3** – Rauschreduktion mit automatischem Rauschprofil, adaptiver Leveler,
Loudness-Normalisierung (ITU-R BS.1770-4 / EBU R128) und True-Peak-Limiter, siehe [PLAN.md](PLAN.md).

Kette: Rauschreduktion → Leveler → Normalisierung → True-Peak-Limiter. Das Rauschprofil wird automatisch
aus den leisesten stationären Sprechpausen jeder Datei bestimmt (Fallback: Minimum Statistics).
Der Leveler gleicht unterschiedlich laute Sprecher und Phrasen an. Die Wechsel liegen in den Pausen,
und Pausen werden nicht angehoben.

## Bauen

```bash
cargo build --release
```

Das Binary liegt danach unter `target/release/leveled-audio`.

## Benutzung

```bash
# Ordner rekursiv auf −16 LUFS / −1 dBTP bringen
leveled-audio process aufnahmen/ -r -o fertig/

# EBU R128 (−23 LUFS), 24 bit, mit JSON-Report
leveled-audio process interview.wav -o fertig/ --preset ebu-r128 --bit-depth 24 --report-json report.json

# Kontrolle: entfernte Anteile als <name>.delta.wav, Profile speichern
leveled-audio process aufnahmen/ -o fertig/ --export-delta --save-profiles profile/

# Ein Profil (z. B. aus einer Raumton-Aufnahme) für alle Dateien erzwingen
leveled-audio profile learn raumton.wav -o studio.json
leveled-audio process aufnahmen/ -o fertig/ --noise-profile studio.json

# Leveler stärker (Default 60 %) oder ganz aus
leveled-audio process interview.wav -o fertig/ --leveler-strength 90
leveled-audio process interview.wav -o fertig/ --no-leveler

# Stärker entrauschen, oder Rauschreduktion ganz aus
leveled-audio process interview.wav -o fertig/ --reduction-db 18
leveled-audio process interview.wav -o fertig/ --no-denoise

# Nur messen (inkl. Rauschpegel und Profil-Konfidenz)
leveled-audio analyze aufnahmen/*.wav

# Eigenes Preset als Vorlage ausgeben
leveled-audio preset show podcast-stereo > mein-preset.toml
```

Wichtige Eigenschaften:

- Eingabe: WAV/RF64, PCM 16/24/32 bit int und 32 bit float, Mono/Stereo.
- Ausgabe: 16/24 bit oder 32 bit float. Dateien über 4 GiB werden automatisch als RF64 geschrieben.
- Vorhandene Zieldateien werden nur mit `--overwrite` ersetzt, Eingabedateien nie.
- Ctrl+C bricht sauber ab und löscht Teildateien (Exit-Code 130).
- Exit-Codes: 0 = ok, 1 = mindestens eine Datei fehlgeschlagen, 2 = Bedienfehler, 130 = Abbruch.
- Deterministisch: gleiche Eingabe und Einstellungen ergeben byte-identische Ausgabe.

## Entwicklung

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Lizenz: MIT OR Apache-2.0.
