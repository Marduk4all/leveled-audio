# leveled-audio (Arbeitstitel)

Batch-Verarbeitung von Sprachaufnahmen (WAV) per Kommandozeile – offline, ohne Laufzeitabhängigkeiten.

**Stand: Phase 1** – Loudness-Normalisierung (ITU-R BS.1770-4 / EBU R128) und True-Peak-Limiter.
Rauschreduktion (Spectral Subtract) und Leveler folgen in Phase 2 und 3, siehe [PLAN.md](PLAN.md).

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

# Nur messen
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
