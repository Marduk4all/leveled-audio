//! Eingaben sammeln und Ausgabepfade planen (inkl. Überschreibschutz).

use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct InputFile {
    pub path: PathBuf,
    /// Relativer Pfad unterhalb des angegebenen Ordners (bzw. nur Dateiname).
    pub rel: PathBuf,
    pub display: String,
}

pub enum Job {
    Process { input: InputFile, output: PathBuf },
    Skip { input: InputFile, output: PathBuf },
}

impl Job {
    pub fn display(&self) -> &str {
        match self {
            Job::Process { input, .. } | Job::Skip { input, .. } => &input.display,
        }
    }
}

fn is_wav(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("wave"))
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("Ordner {} lesen", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue; // versteckte Dateien, macOS-"._"-Ressourcen
        }
        if p.is_dir() {
            if recursive {
                walk(&p, recursive, out)?;
            }
        } else if is_wav(&p) {
            out.push(p);
        }
    }
    Ok(())
}

/// Dateien und Ordner zu einer sortierten, doppelfreien Liste auflösen.
pub fn collect_inputs(args: &[PathBuf], recursive: bool) -> Result<Vec<InputFile>> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for arg in args {
        if arg.is_dir() {
            let mut found = Vec::new();
            walk(arg, recursive, &mut found)?;
            if found.is_empty() {
                eprintln!(
                    "Hinweis: keine WAV-Dateien in {}{}",
                    arg.display(),
                    if recursive {
                        ""
                    } else {
                        " (Unterordner mit -r)"
                    }
                );
            }
            for p in found {
                let rel = p.strip_prefix(arg).unwrap_or(&p).to_path_buf();
                push_unique(&mut files, &mut seen, p, rel)?;
            }
        } else if arg.is_file() {
            let rel = PathBuf::from(arg.file_name().context("ungültiger Dateiname")?);
            push_unique(&mut files, &mut seen, arg.clone(), rel)?;
        } else {
            bail!("Eingabe nicht gefunden: {}", arg.display());
        }
    }
    if files.is_empty() {
        bail!("keine WAV-Dateien gefunden");
    }
    Ok(files)
}

fn push_unique(
    files: &mut Vec<InputFile>,
    seen: &mut HashSet<PathBuf>,
    path: PathBuf,
    rel: PathBuf,
) -> Result<()> {
    let canon = path
        .canonicalize()
        .with_context(|| format!("{} auflösen", path.display()))?;
    if seen.insert(canon) {
        let display = path.display().to_string();
        files.push(InputFile { path, rel, display });
    }
    Ok(())
}

/// Dateiname aus dem Benennungsschema.
pub fn render_name(
    template: &str,
    stem: &str,
    ext: &str,
    preset: &str,
    lufs: f64,
) -> Result<String> {
    let name = template
        .replace("{stem}", stem)
        .replace("{ext}", ext)
        .replace("{preset}", preset)
        .replace("{lufs}", &format!("{lufs}"));
    if name.contains('/') || name.contains('\\') || name.is_empty() || name == "." || name == ".." {
        bail!("Benennungsschema ergibt ungültigen Dateinamen „{name}“");
    }
    if let Some(open) = name.find('{')
        && name[open..].contains('}')
    {
        bail!("unbekannter Platzhalter im Benennungsschema „{template}“");
    }
    if Path::new(&name).extension().is_none() {
        return Ok(format!("{name}.wav"));
    }
    Ok(name)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Ausgabepfade bestimmen und alle Konflikte **vor** dem Start prüfen.
pub fn plan_outputs(
    files: Vec<InputFile>,
    out_dir: &Path,
    template: &str,
    preset: &str,
    lufs: f64,
    overwrite: bool,
    skip_existing: bool,
) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    let mut targets: HashMap<PathBuf, String> = HashMap::new();
    let mut existing = Vec::new();
    for f in files {
        let stem = f.rel.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
        let ext = f
            .rel
            .extension()
            .and_then(|s| s.to_str())
            .map_or(String::from(".wav"), |e| format!(".{e}"));
        let name = render_name(template, stem, &ext, preset, lufs)?;
        let parent = f.rel.parent().unwrap_or(Path::new(""));
        let output = out_dir.join(parent).join(name);

        // Eingaben werden nie überschrieben – auch nicht mit --overwrite.
        if output.exists() && same_file(&output, &f.path) {
            bail!(
                "Ausgabe {} wäre die Eingabedatei selbst; anderen Ausgabeordner oder anderes Benennungsschema wählen",
                output.display()
            );
        }
        if let Some(prev) = targets.insert(output.clone(), f.display.clone()) {
            bail!(
                "{} und {} würden beide nach {} geschrieben",
                prev,
                f.display,
                output.display()
            );
        }
        if output.exists() {
            if skip_existing {
                jobs.push(Job::Skip { input: f, output });
                continue;
            }
            if !overwrite {
                existing.push(output.display().to_string());
            }
        }
        jobs.push(Job::Process { input: f, output });
    }
    if !existing.is_empty() {
        let shown: Vec<_> = existing.iter().take(5).cloned().collect();
        bail!(
            "{} Zieldatei(en) existieren bereits (z. B. {}). Mit --overwrite überschreiben oder mit --skip-existing überspringen",
            existing.len(),
            shown.join(", ")
        );
    }
    for j in &jobs {
        if let Job::Process { output, .. } = j
            && let Some(dir) = output.parent()
        {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("Ordner {} anlegen", dir.display()))?;
        }
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_tokens() {
        assert_eq!(
            render_name(
                "{stem}_{preset}_{lufs}{ext}",
                "talk",
                ".wav",
                "ebu-r128",
                -23.0
            )
            .unwrap(),
            "talk_ebu-r128_-23.wav"
        );
        assert_eq!(
            render_name("{stem}_x", "a", ".WAV", "p", -16.0).unwrap(),
            "a_x.wav"
        );
        assert!(render_name("../{stem}", "a", ".wav", "p", -16.0).is_err());
        assert!(render_name("{stme}{ext}", "a", ".wav", "p", -16.0).is_err());
    }
}
