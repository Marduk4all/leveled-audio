//! Beschreibung aller Einstellungen für Oberflächen (PLAN Phase 6).
//!
//! Eine Tabelle: Schlüssel (wie in Presets und `--set`), Gruppe, Bezeichnung,
//! Art mit erlaubtem Bereich und Kurzbeschreibung. Die Bereiche entsprechen
//! [`Settings::validate`]; ein Test stellt sicher, dass beides übereinstimmt
//! und jeder Schlüssel der Settings hier vorkommt.

use crate::settings::Settings;

/// Art eines Parameters samt erlaubtem Bereich.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    Bool,
    /// Zahl im Bereich `min..=max`. `percent`: intern 0–1, angezeigt 0–100 %.
    Float {
        min: f64,
        max: f64,
        unit: &'static str,
        decimals: usize,
        percent: bool,
    },
    /// Auswahl aus festen Werten: (TOML-Wert, Bezeichnung).
    Choice(&'static [(&'static str, &'static str)]),
    /// Ganzzahl aus einer festen Liste.
    IntChoice(&'static [i64]),
    /// FFT-Größe: `"auto"` oder Zweierpotenz.
    FftSize,
}

#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    /// Schlüssel wie in Presets, z. B. `denoise.reduction_db`.
    pub key: &'static str,
    /// Gruppe (Stufe der Kette), für die Anzeige.
    pub group: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    /// Kurzbeschreibung: was sich ändert (PLAN 4.5).
    pub help: &'static str,
    /// Wichtiger Regler (oben in der Gruppe), sonst Feineinstellung.
    pub primary: bool,
}

const fn float(min: f64, max: f64, unit: &'static str, decimals: usize) -> ParamKind {
    ParamKind::Float {
        min,
        max,
        unit,
        decimals,
        percent: false,
    }
}

const PERCENT: ParamKind = ParamKind::Float {
    min: 0.0,
    max: 1.0,
    unit: "%",
    decimals: 0,
    percent: true,
};

macro_rules! p {
    ($key:expr, $group:expr, $label:expr, $kind:expr, $primary:expr, $help:expr) => {
        ParamSpec {
            key: $key,
            group: $group,
            label: $label,
            kind: $kind,
            help: $help,
            primary: $primary,
        }
    };
}

/// Gruppen in Reihenfolge der Kette.
pub const PARAM_GROUPS: [&str; 9] = [
    "Hochpass",
    "Rauschreduktion",
    "Knackser",
    "Leveler",
    "Pausen-Absenkung",
    "Kompressor",
    "Lautheit",
    "Limiter",
    "Ausgabe",
];

/// Alle Einstellungen.
pub const PARAMS: &[ParamSpec] = &[
    // ---- Hochpass
    p!(
        "highpass.enabled",
        "Hochpass",
        "Hochpass an",
        ParamKind::Bool,
        true,
        "Entfernt Rumpeln und Trittschall unterhalb der Grenzfrequenz."
    ),
    p!(
        "highpass.frequency_hz",
        "Hochpass",
        "Grenzfrequenz",
        float(20.0, 200.0, "Hz", 0),
        true,
        "Höher = schlanker; ab ≈ 100 Hz verlieren tiefe Stimmen Wärme."
    ),
    // ---- Rauschreduktion
    p!(
        "denoise.enabled",
        "Rauschreduktion",
        "Rauschreduktion an",
        ParamKind::Bool,
        true,
        "Rauschprofil automatisch aus den ruhigsten Pausen, spektral abziehen. Aus: auch keine Knackser-Absenkung."
    ),
    p!(
        "denoise.reduction_db",
        "Rauschreduktion",
        "Max. Absenkung",
        float(0.0, 40.0, "dB", 1),
        true,
        "Weniger Rauschen; ab ≈ 15–18 dB drohen „blubbernde“ Artefakte und dumpfere Sprache."
    ),
    p!(
        "denoise.gain_rule",
        "Rauschreduktion",
        "Gain-Regel",
        ParamKind::Choice(&[
            ("wiener-dd", "Wiener (weniger Musical Noise)"),
            ("subtract", "Power Subtraction (klassisch)")
        ]),
        false,
        "Wiener-Filter mit Decision-Directed-Schätzung klingt ruhiger."
    ),
    p!(
        "denoise.oversubtraction",
        "Rauschreduktion",
        "Überschätzung",
        float(0.5, 4.0, "×", 2),
        false,
        "Wie stark das Rauschprofil überschätzt wird. Höher = gründlicher, aber mehr Artefakte."
    ),
    p!(
        "denoise.floor",
        "Rauschreduktion",
        "Spectral Floor",
        float(0.0, 1.0, "", 2),
        false,
        "Untergrenze relativ zum Rauschen (nur bei Power Subtraction)."
    ),
    p!(
        "denoise.release_ms",
        "Rauschreduktion",
        "Rückstellzeit",
        float(0.0, 1000.0, "ms", 0),
        false,
        "Wie schnell die Absenkung nach Sprache zurückkehrt. Länger = ruhiger, aber verschmierter."
    ),
    p!(
        "denoise.freq_smooth_oct",
        "Rauschreduktion",
        "Frequenzglättung",
        float(0.0, 1.0, "Okt.", 2),
        false,
        "Glättet den Gain über die Frequenz; mindert Musical Noise, macht die Absenkung gröber."
    ),
    p!(
        "denoise.fft_size",
        "Rauschreduktion",
        "FFT-Größe",
        ParamKind::FftSize,
        false,
        "auto ≈ 43 ms. Größer = feinere Frequenzauflösung, aber verwaschene Transienten."
    ),
    p!(
        "denoise.stereo_link",
        "Rauschreduktion",
        "Stereo gekoppelt",
        ParamKind::Bool,
        false,
        "Gleicher Gain für beide Kanäle; erhält das Stereobild."
    ),
    p!(
        "denoise.profile.min_duration_s",
        "Rauschreduktion",
        "Profil: Mindestdauer",
        float(0.1, 60.0, "s", 1),
        false,
        "So viel Pausenmaterial braucht ein automatisches Profil mindestens."
    ),
    p!(
        "denoise.profile.target_duration_s",
        "Rauschreduktion",
        "Profil: Zieldauer",
        float(0.1, 60.0, "s", 1),
        false,
        "Ab dieser Menge Pausenmaterial gilt das Profil als voll vertrauenswürdig (≥ Mindestdauer)."
    ),
    p!(
        "denoise.profile.statistic",
        "Rauschreduktion",
        "Profil: Mittelung",
        ParamKind::Choice(&[
            ("trimmed", "getrimmtes Mittel"),
            ("mean", "Mittel"),
            ("median", "Median")
        ]),
        false,
        "Wie die Pausen zum Profil gemittelt werden."
    ),
    p!(
        "denoise.profile.min_confidence",
        "Rauschreduktion",
        "Profil: Mindestkonfidenz",
        float(0.0, 1.0, "", 2),
        false,
        "Darunter dient Minimum Statistics als Ersatz für das Profil."
    ),
    // ---- Knackser
    p!(
        "knock.enabled",
        "Knackser",
        "Knackser erkennen",
        ParamKind::Bool,
        true,
        "Griffgeräusche und Knackser erkennen; sie werden dann nicht mitgelevelt."
    ),
    p!(
        "knock.reduction_db",
        "Knackser",
        "Absenkung der Tiefen",
        float(0.0, 40.0, "dB", 1),
        true,
        "Knackser leiser; gleichzeitige Sprache klingt dort kurz dünner. 0 = nur erkennen."
    ),
    p!(
        "knock.max_speech_to_low_db",
        "Knackser",
        "Erkennungsschwelle",
        float(-30.0, -5.0, "dB", 1),
        false,
        "Sprachband so weit unter den Tiefen ⇒ Störgeräusch. Höher erkennt mehr, riskiert aber tiefe Sprachlaute."
    ),
    // ---- Leveler
    p!(
        "leveler.enabled",
        "Leveler",
        "Leveler an",
        ParamKind::Bool,
        true,
        "Gleicht unterschiedlich laute Sprecher und Phrasen an."
    ),
    p!(
        "leveler.strength",
        "Leveler",
        "Stärke",
        ParamKind::Float {
            min: 0.0,
            max: 1.3,
            unit: "%",
            decimals: 0,
            percent: true,
        },
        true,
        "100 % = Sprecher gleich laut, 0 % = Unterschiede bleiben. Über 100 % wird überkorrigiert: Leise Stellen landen etwas über den lauten, das gleicht die Glättung des Levelers und große Abstandswechsel stärker aus. Höher klingt gleichmäßiger, aber weniger lebendig."
    ),
    p!(
        "leveler.max_boost_db",
        "Leveler",
        "Max. Anhebung",
        float(0.0, 20.0, "dB", 1),
        false,
        "Größte Anhebung leiser Phrasen."
    ),
    p!(
        "leveler.max_cut_db",
        "Leveler",
        "Max. Absenkung",
        float(0.0, 20.0, "dB", 1),
        false,
        "Größte Absenkung lauter Phrasen."
    ),
    p!(
        "leveler.pause_max_boost_db",
        "Leveler",
        "Max. Gain in Pausen",
        float(-10.0, 10.0, "dB", 1),
        false,
        "Obergrenze des Gains in langen Pausen; 0 = Pausen werden nicht angehoben."
    ),
    p!(
        "leveler.slew_db_per_s",
        "Leveler",
        "Max. Änderung in Phrasen",
        float(0.5, 10.0, "dB/s", 1),
        false,
        "Wie schnell sich der Gain innerhalb einer Phrase ändern darf. Höher = schneller, aber Pumpgefahr."
    ),
    // ---- Pausen-Absenkung
    p!(
        "gate.enabled",
        "Pausen-Absenkung",
        "Pausen absenken",
        ParamKind::Bool,
        true,
        "Aus = natürlicher Raumton bleibt. An = Pausen fast still; kann an Pausengrenzen „atmen“."
    ),
    p!(
        "gate.floor_dbfs",
        "Pausen-Absenkung",
        "Pegel der Pausen",
        float(-90.0, -30.0, "dBFS", 0),
        true,
        "Zielpegel der Pausen in der fertigen Datei."
    ),
    p!(
        "gate.max_attenuation_db",
        "Pausen-Absenkung",
        "Max. Absenkung",
        float(0.0, 60.0, "dB", 0),
        false,
        "Höchstens so viel zusätzliche Absenkung."
    ),
    p!(
        "gate.min_pause_ms",
        "Pausen-Absenkung",
        "Mindestpause",
        float(100.0, 2000.0, "ms", 0),
        false,
        "Kürzere Pausen bleiben unverändert (sonst wird zwischen Wörtern zerhackt)."
    ),
    p!(
        "gate.hold_ms",
        "Pausen-Absenkung",
        "Haltezeit",
        float(0.0, 500.0, "ms", 0),
        false,
        "Halten nach dem letzten Laut, bevor ausgeblendet wird. Länger schützt Ausklänge."
    ),
    p!(
        "gate.fade_out_ms",
        "Pausen-Absenkung",
        "Ausblenden",
        float(10.0, 1000.0, "ms", 0),
        false,
        "Ausblenden in die Pause. Länger = sanfter, aber später leise."
    ),
    p!(
        "gate.fade_in_ms",
        "Pausen-Absenkung",
        "Aufblenden",
        float(10.0, 500.0, "ms", 0),
        false,
        "Aufblenden vor dem nächsten Wort."
    ),
    p!(
        "gate.pre_open_ms",
        "Pausen-Absenkung",
        "Vorab öffnen",
        float(0.0, 200.0, "ms", 0),
        false,
        "So viel früher vor Sprachbeginn ist die Absenkung ganz aufgehoben."
    ),
    // ---- Kompressor
    p!(
        "compressor.enabled",
        "Kompressor",
        "Kompressor an",
        ParamKind::Bool,
        true,
        "Fängt kurze laute Stellen (Lachen, Ausrufe) ab, bevor der Limiter hart begrenzt."
    ),
    p!(
        "compressor.amount",
        "Kompressor",
        "Stärke",
        PERCENT,
        true,
        "40 % ≈ 1,5:1 (natürlich), 60 % ≈ 2:1 (kräftig). Höher = dichter, zu hoch klingt gepresst."
    ),
    p!(
        "compressor.threshold_above_ref_db",
        "Kompressor",
        "Schwelle über Sprache",
        float(0.0, 20.0, "dB", 1),
        false,
        "Schwelle relativ zum Sprachpegel der Datei. Niedriger erfasst auch normale Silben."
    ),
    p!(
        "compressor.attack_ms",
        "Kompressor",
        "Ansprechzeit",
        float(0.5, 50.0, "ms", 1),
        false,
        "Wie schnell der Kompressor zupackt."
    ),
    p!(
        "compressor.release_ms",
        "Kompressor",
        "Rückstellzeit",
        float(20.0, 1000.0, "ms", 0),
        false,
        "Wie schnell er wieder loslässt. Kürzer = lebendiger, aber Pumpgefahr."
    ),
    // ---- Lautheit
    p!(
        "loudness.enabled",
        "Lautheit",
        "Normalisieren",
        ParamKind::Bool,
        true,
        "Bringt die integrierte Lautheit (EBU R128) auf das Ziel."
    ),
    p!(
        "loudness.target_lufs",
        "Lautheit",
        "Ziel-Lautheit",
        float(-70.0, -5.0, "LUFS", 1),
        true,
        "Gesamtlautheit. Plattformen normalisieren meist auf −14 bis −16 LUFS."
    ),
    p!(
        "loudness.mode",
        "Lautheit",
        "Modus",
        ParamKind::Choice(&[
            ("per-file", "pro Datei"),
            ("album", "Album (ein Gain für alle)")
        ]),
        true,
        "Album nur für Folgen einer Produktion; sonst bleiben leise Dateien leise."
    ),
    // ---- Limiter
    p!(
        "limiter.enabled",
        "Limiter",
        "Limiter an",
        ParamKind::Bool,
        true,
        "True-Peak-Limiter mit Oversampling."
    ),
    p!(
        "limiter.ceiling_dbtp",
        "Limiter",
        "Obergrenze",
        float(-20.0, 0.0, "dBTP", 1),
        true,
        "Obergrenze für Spitzen. Höher entlastet den Limiter, riskiert aber Verzerrungen nach MP3/AAC."
    ),
    p!(
        "limiter.lookahead_ms",
        "Limiter",
        "Vorausschau",
        float(0.5, 50.0, "ms", 1),
        false,
        "Wie weit der Limiter vorausschaut."
    ),
    p!(
        "limiter.release_ms",
        "Limiter",
        "Rückstellzeit",
        float(1.0, 2000.0, "ms", 0),
        false,
        "Wie schnell der Limiter wieder loslässt."
    ),
    p!(
        "limiter.oversampling",
        "Limiter",
        "Oversampling",
        ParamKind::IntChoice(&[4, 8, 16]),
        false,
        "Genauigkeit der Spitzenerkennung zwischen den Samples."
    ),
    // ---- Ausgabe
    p!(
        "output.bit_depth",
        "Ausgabe",
        "Bittiefe",
        ParamKind::Choice(&[
            ("same", "wie Eingabe"),
            ("16", "16 bit"),
            ("24", "24 bit"),
            ("32f", "32 bit float")
        ]),
        true,
        "„wie Eingabe“ macht aus 32-bit-int 24 bit."
    ),
    p!(
        "output.dither",
        "Ausgabe",
        "Dither",
        ParamKind::Choice(&[
            ("auto", "automatisch (nur 16 bit)"),
            ("on", "an"),
            ("off", "aus")
        ]),
        false,
        "TPDF-Dither beim Verringern der Bittiefe."
    ),
];

/// Parameter zu einem Schlüssel.
pub fn param(key: &str) -> Option<&'static ParamSpec> {
    PARAMS.iter().find(|p| p.key == key)
}

/// Einstellungen als TOML-Baum (für den Zugriff über Schlüssel).
pub fn settings_table(s: &Settings) -> toml::Table {
    toml::Table::try_from(s).expect("Settings sind serialisierbar")
}

/// Wert zu einem Schlüssel aus einem TOML-Baum der Einstellungen.
pub fn lookup<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let mut cur = table.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_table()?.get(p)?;
    }
    Some(cur)
}

impl Settings {
    /// Wert zu einem Schlüssel, z. B. `leveler.strength`.
    pub fn get_value(&self, key: &str) -> Option<toml::Value> {
        lookup(&settings_table(self), key).cloned()
    }

    /// Wert setzen. Unbekannter Schlüssel oder falscher Typ ⇒ Fehler, die
    /// Einstellungen bleiben dann unverändert. Bereiche prüft [`Settings::validate`].
    pub fn set_value(&mut self, key: &str, value: toml::Value) -> std::result::Result<(), String> {
        let mut patch = toml::Table::new();
        let parts: Vec<&str> = key.split('.').collect();
        let mut cur = &mut patch;
        for p in &parts[..parts.len() - 1] {
            cur = cur
                .entry(p.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .expect("Tabelle");
        }
        cur.insert(parts[parts.len() - 1].to_string(), value);
        *self = self.layered(patch).map_err(|e| format!("{key}: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alle Blatt-Schlüssel eines TOML-Baums.
    fn leaves(t: &toml::Table, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                toml::Value::Table(sub) => leaves(sub, &key, out),
                _ => out.push(key),
            }
        }
    }

    #[test]
    fn every_setting_is_described_exactly_once() {
        let mut keys = Vec::new();
        leaves(&settings_table(&Settings::default()), "", &mut keys);
        for k in &keys {
            assert!(param(k).is_some(), "Schlüssel ohne Beschreibung: {k}");
        }
        for p in PARAMS {
            assert!(
                keys.iter().any(|k| k == p.key),
                "unbekannter Schlüssel {}",
                p.key
            );
            assert!(PARAM_GROUPS.contains(&p.group), "Gruppe {}", p.group);
            assert!(!p.help.is_empty() && !p.label.is_empty());
            assert_eq!(PARAMS.iter().filter(|q| q.key == p.key).count(), 1);
        }
    }

    #[test]
    fn ranges_match_validation() {
        for p in PARAMS {
            let ok = |v: toml::Value| {
                let mut s = Settings::default();
                // Abhängigkeit: Zieldauer ≥ Mindestdauer.
                if p.key == "denoise.profile.min_duration_s" {
                    s.denoise.profile.target_duration_s = 60.0;
                }
                if p.key == "denoise.profile.target_duration_s" {
                    s.denoise.profile.min_duration_s = 0.1;
                }
                s.set_value(p.key, v).is_ok() && s.validate().is_ok()
            };
            match p.kind {
                ParamKind::Float { min, max, .. } => {
                    assert!(ok(min.into()), "{}: Minimum {min} abgelehnt", p.key);
                    assert!(ok(max.into()), "{}: Maximum {max} abgelehnt", p.key);
                    let eps = (max - min) * 1e-3;
                    assert!(!ok((min - eps).into()), "{}: unter {min} akzeptiert", p.key);
                    assert!(!ok((max + eps).into()), "{}: über {max} akzeptiert", p.key);
                }
                ParamKind::Bool => {
                    assert!(ok(true.into()) && ok(false.into()), "{}", p.key);
                }
                ParamKind::Choice(c) => {
                    for (v, _) in c {
                        assert!(ok((*v).into()), "{}: {v}", p.key);
                    }
                    assert!(!ok("unbekannt".into()), "{}", p.key);
                }
                ParamKind::IntChoice(c) => {
                    for v in c {
                        assert!(ok((*v).into()), "{}: {v}", p.key);
                    }
                    assert!(!ok(5.into()), "{}", p.key);
                }
                ParamKind::FftSize => {
                    assert!(ok("auto".into()));
                    for n in [512, 1024, 2048, 4096, 8192] {
                        assert!(ok(n.into()), "{n}");
                    }
                    assert!(!ok(1000.into()));
                }
            }
        }
    }

    #[test]
    fn get_and_set_values() {
        let mut s = Settings::default();
        assert_eq!(s.get_value("leveler.strength"), Some(1.3.into()));
        s.set_value("leveler.strength", 0.25.into()).unwrap();
        assert_eq!(s.leveler.strength, 0.25);
        s.set_value("denoise.profile.statistic", "median".into())
            .unwrap();
        assert_eq!(
            s.get_value("denoise.profile.statistic"),
            Some("median".into())
        );
        let before = s.clone();
        assert!(s.set_value("leveler.staerke", 0.3.into()).is_err());
        assert!(s.set_value("leveler.strength", "viel".into()).is_err());
        assert_eq!(s, before);
    }
}
