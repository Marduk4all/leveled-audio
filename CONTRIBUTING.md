# Contributing to Leveled Audio

Thanks for your interest! Bug reports, ideas, sample recordings that sound wrong, documentation fixes and code are all welcome. You can write in **English or German**.

## Ways to help

- **Report a bug:** [open an issue](https://github.com/Marduk4all/leveled-audio/issues/new/choose) with the bug template. Include your operating system, the app version (`leveled-audio --version`), the preset or settings you used, and the JSON report (`--report-json report.json` or "Save JSON report…" in the app) if you have one.
- **Audio that doesn't sound right:** describe what you hear (pumping, bubbling, too quiet, noise coming and going …) and, if you can, share a short excerpt. Only share recordings you are allowed to share.
- **Questions and ideas:** use [Discussions](https://github.com/Marduk4all/leveled-audio/discussions), e.g. "Which preset for my video?" or "Would feature X make sense?".
- **Documentation:** the user guide exists in English ([docs/GUIDE.md](docs/GUIDE.md)) and German ([docs/ANLEITUNG.md](docs/ANLEITUNG.md)). Please keep both in sync, or say in the pull request that the other language still needs an update.

## Development setup

You need [Rust](https://rustup.rs). The toolchain version is pinned in `rust-toolchain.toml` and installed automatically.

On Linux, the desktop app needs a few system packages:

```bash
sudo apt-get install -y libgtk-3-dev libxkbcommon-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

Build and run:

```bash
cargo run --release -p leveled-audio-gui          # desktop app
cargo run --release -p leveled-audio-cli -- --help # command line
```

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on Linux, macOS (Apple Silicon and Intel) and Windows.

Guidelines:

- **Keep changes focused:** one topic per pull request.
- **Add a test** for new behavior or a fixed bug. Audio behavior is tested with synthetic signals from `leveled-audio-testkit`, not with real recordings.
- **Determinism matters:** the same input and settings must give byte-identical output.
- **User-facing text:** the desktop app is translated in `crates/leveled-audio-gui/src/i18n.rs`. A test fails if an English text is missing.
- **Accessibility:** every control in the desktop app needs a name for screen readers. Link it to its visible label with `.labelled_by(label.id)`. The test `crates/leveled-audio-gui/tests/accessibility.rs` checks this for all settings in both languages.
- **Settings:** a new setting needs an entry in `crates/leveled-audio-pipeline/src/params.rs` (label, range, help text) and in the settings table of both guides.
- **Screenshots:** if you change the app's look, regenerate them with `cargo run --release -p leveled-audio-gui --example screenshots -- <workdir> docs/images`.

## Project layout

| Crate | Purpose |
|---|---|
| `leveled-audio-dsp` | DSP building blocks: spectral noise reduction, oversampling, limiter, dither |
| `leveled-audio-analysis` | loudness measurement, voice activity detection, noise profiles, leveler curve |
| `leveled-audio-io` | WAV/RF64 reading and writing |
| `leveled-audio-pipeline` | processing chain, settings, presets, reports |
| `leveled-audio-cli` | command-line tool `leveled-audio` |
| `leveled-audio-gui` | desktop app (egui) |
| `leveled-audio-testkit` | test signals and measurement helpers |

Design decisions and measurements are documented in [PLAN.md](PLAN.md) (German).

## License of contributions

Unless you state otherwise, any contribution you submit is dual-licensed under MIT OR Apache-2.0, like the project itself, without additional terms.

## Code of conduct

Please be kind and constructive. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
