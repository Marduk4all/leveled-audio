# Leveled Audio

**Automatic audio leveling, noise reduction and loudness normalization for spoken-word recordings. Built for podcasts, video production, interviews and voice-over. Free, open source and offline.**

[![Release](https://img.shields.io/github/v/release/Marduk4all/leveled-audio)](https://github.com/Marduk4all/leveled-audio/releases/latest)
[![CI](https://github.com/Marduk4all/leveled-audio/actions/workflows/ci.yml/badge.svg)](https://github.com/Marduk4all/leveled-audio/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)

**English** · [Deutsch](README.de.md)

Leveled Audio turns uneven speech recordings into finished, consistently loud audio in one pass:

- **Even speaker levels:** quiet and loud speakers end up at the same level, even when someone moves between 1 m and 5 cm from the microphone.
- **Less background noise:** room noise is reduced, and handling noise and knocks are removed.
- **Loudness on target:** the result hits a loudness target in LUFS, e.g. −16 LUFS for podcasts or −23 LUFS for EBU R128 broadcast, without clipping (true-peak limiter).
- **Batch processing:** a whole folder of WAV files at once, on your own computer. Nothing is uploaded, and there is no account and no subscription.

You can use it as a **desktop app** (macOS, Windows, Linux) or as the **command-line tool** `leveled-audio` for scripts and automated workflows.

<p align="center">
  <img src="docs/images/gui-settings-en.png" width="420" alt="Leveled Audio desktop app: add WAV files, choose a preset, adjust leveler strength">
  <img src="docs/images/gui-result-en.png" width="420" alt="Leveled Audio desktop app: export result with loudness (LUFS) and true peak per file">
</p>

<p align="center">
  <a href="https://github.com/Marduk4all/leveled-audio/releases/latest"><b>⬇ Download the latest release</b></a> ·
  <a href="https://marduk4all.github.io/leveled-audio/">Website</a> ·
  <a href="docs/GUIDE.md">User guide</a> ·
  <a href="#quick-start">Quick start</a>
</p>

---

## Use cases

### 🎙 Podcasts
- **Interviews and round tables:** guests on different microphones, or remote guests recorded at very different levels, end up at the same loudness.
- **Pauses:** pauses stay natural, or nearly silent with the `podcast-kraeftig` preset.
- **Loudness target:** −16 LUFS by default, as recommended by most podcast platforms. Use `podcast-mono` (−19 LUFS) for mono shows.
- **Whole seasons:** album mode applies one shared gain to all episodes, so they sound the same to each other.

### 🎬 Video production (YouTube, social media, documentaries, corporate video)
- **Dialogue and voice-over:** fix levels before the final mix, and clean up interview audio from lavalier or camera microphones.
- **Changing distance:** a presenter who walks around or turns away from the microphone stays at a steady level.
- **Delivery targets:** −14 LUFS is common for YouTube and streaming, −23 LUFS (EBU R128) for broadcast. Choose a target with one setting.
- **Workflow:** export the dialogue track as WAV from your editor (or with ffmpeg, see below), process it in a batch, and import it back into the timeline.

### 🎓 Lectures, courses and webinars
- **Different speakers:** speakers who are hard to hear and speakers who are too loud are evened out.
- **Room recordings:** room noise is reduced in lecture-hall recordings and screen-recording narration.

### 📚 Audiobooks, voice-over and narration
- **Consistent chapters:** many recording sessions end up at the same loudness, with a controlled peak level.

### 📻 Radio and broadcast
- **EBU R128:** the `ebu-r128` preset (−23 LUFS) keeps the natural room tone.
- **Archiving:** a JSON report records all measurements for documentation and QA.

---

## Features

| | |
|---|---|
| **Adaptive leveler** | Evens out speakers and phrases of different loudness. Gain changes happen in pauses, and pauses are not raised. The strength can be set up to 130 % (overcorrection) for large changes in microphone distance. |
| **Noise reduction** | Spectral noise reduction with a noise profile found automatically in the quietest pauses of each file, or learned once from a room-tone recording. |
| **Handling-noise removal** | Detects low-frequency knocks and microphone handling noise, lowers them, and keeps the leveler from boosting them. |
| **Compressor** | Gently catches short loud moments such as laughter and exclamations. |
| **Pause gate** (optional) | Lowers pauses to a target level for a tight, "radio" sound. |
| **Loudness normalization** | ITU-R BS.1770-4 / EBU R128 integrated loudness, per file or as an album with one shared gain. |
| **True-peak limiter** | 8× oversampling, default −1 dBTP, safe for MP3/AAC encoding. |
| **High-pass filter** | Removes rumble and footstep noise. |
| **Presets** | `podcast-stereo`, `podcast-mono`, `podcast-kraeftig` (punchy), `ebu-r128`, plus your own TOML presets that the app and the command line share. |
| **Reports** | Console table and JSON report (with schema) with loudness, true peak, LRA, noise level and gain changes per file. |
| **Safe** | Never modifies originals, writes no half-finished files, gives byte-identical results for the same input, and handles files several hours long with nearly constant memory use. |
| **Formats** | WAV and RF64 input and output: 16/24/32-bit int, 32-bit float, mono and stereo, common sample rates. |

**Processing chain:** high-pass → noise reduction + handling-noise reduction → leveler (+ pause gate) → compressor → loudness normalization → true-peak limiter.

### Loudness targets at a glance

| Destination | Target | How |
|---|---|---|
| Podcast (stereo) | −16 LUFS | preset `podcast-stereo` (default) |
| Podcast (mono) | −19 LUFS | preset `podcast-mono` |
| YouTube / streaming video | −14 LUFS | `--target-lufs -14` or the "Target loudness" setting |
| Broadcast (EBU R128) | −23 LUFS | preset `ebu-r128` |
| Broadcast (ATSC A/85, US) | −24 LUFS | `--target-lufs -24` |

Platform recommendations change over time, so check your platform's current delivery specification.

---

## Quick start

### Which file do I need?

| Your computer | File name ends in |
|---|---|
| Mac (any model) | `-macos.tar.gz` (v0.1.0: `-universal-apple-darwin.tar.gz`) |
| Windows 10/11 | `-windows-x64.zip` (v0.1.0: `-x86_64-pc-windows-msvc.zip`) |
| Linux desktop | `-linux-x64.tar.gz` (v0.1.0: `-x86_64-unknown-linux-gnu.tar.gz`) |
| Linux server / Docker, command line only | `-linux-x64-cli-only.tar.gz` (v0.1.0: `-x86_64-unknown-linux-musl.tar.gz`) |

### Desktop app

1. **Download** the archive for your system from the [latest release](https://github.com/Marduk4all/leveled-audio/releases/latest) and unpack it.
   - **macOS:** move `Leveled Audio.app` to Applications. The first time, right-click it → Open, because the app is not notarized.
   - **Windows:** start `leveled-audio-gui.exe`. If SmartScreen warns, click "More info" → "Run anyway".
   - **Linux:** start `leveled-audio-gui`.
2. **Add** WAV files or whole folders, or drag them into the window.
3. **Pick a preset** and adjust the settings if needed. Hover over any control for an explanation.
4. **Click Export.** Each file is saved next to its original as `name-leveled-<preset>.wav`. The app asks before overwriting.

The app starts in English. Use **EN | DE** at the top right to switch to German.

### Command line

```bash
# Podcast: a whole folder (recursively) to −16 LUFS / −1 dBTP
leveled-audio process recordings/ -r -o done/

# YouTube video dialogue at −14 LUFS
leveled-audio process dialogue.wav -o done/ --target-lufs -14

# Broadcast: EBU R128 (−23 LUFS), 24 bit, with JSON report
leveled-audio process interview.wav -o done/ --preset ebu-r128 --bit-depth 24 --report-json report.json

# Punchier podcast sound: near-silent pauses, denser
leveled-audio process recordings/ -o done/ --preset podcast-kraeftig

# A whole season with one shared gain (relative differences stay)
leveled-audio process season/ -o done/ --loudness-mode album

# Learn a noise profile from room tone once and use it for all files
leveled-audio profile learn roomtone.wav -o studio.json
leveled-audio process recordings/ -o done/ --noise-profile studio.json

# Measure only: loudness, true peak, LRA, noise level
leveled-audio analyze recordings/*.wav
```

The command-line tool's messages and tables are in German. All options are explained in the [user guide](docs/GUIDE.md#3-commands).

### Video workflow with ffmpeg

Leveled Audio reads and writes WAV. For a video file, take the audio out first, process it, and put it back:

```bash
# 1. Extract the audio as 24-bit WAV
ffmpeg -i video.mp4 -vn -c:a pcm_s24le dialogue.wav

# 2. Level, denoise and normalize to −14 LUFS
leveled-audio process dialogue.wav -o done/ --target-lufs -14

# 3. Put the processed audio back (video stream is copied, not re-encoded)
ffmpeg -i video.mp4 -i done/dialogue.wav -map 0:v -map 1:a -c:v copy -c:a aac -b:a 192k video-leveled.mp4
```

For edited projects, export the dialogue stem from your editor (DaVinci Resolve, Premiere Pro, Final Cut Pro …) as WAV and import the result back.

---

## Documentation

- **[Website](https://marduk4all.github.io/leveled-audio/):** overview, use cases and download.
- **[User guide (English)](docs/GUIDE.md):** installation, every setting with recommendations, presets, album mode, JSON report, FAQ.
- **[Bedienungsanleitung (Deutsch)](docs/ANLEITUNG.md)**
- **[JSON report schema](docs/report.schema.json)**
- **[Development plan](PLAN.md)** (German): design decisions, measurements, roadmap.

## Build from source

You need [Rust](https://rustup.rs) (the version is pinned in `rust-toolchain.toml`).

```bash
cargo build --release
scripts/macos-app.sh        # macOS only: creates target/release/Leveled Audio.app
```

The binaries are then in `target/release/` (`leveled-audio`, `leveled-audio-gui`).

To create a release, push a tag (`git tag v0.2.0 && git push origin v0.2.0`). The workflow builds macOS (universal), Windows and Linux, checks the binaries and publishes them together with `SHA256SUMS.txt`. Release notes are taken from `docs/releases/<tag>.md`.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

To regenerate the screenshots, the app icon and the social preview image:

```bash
cargo run --release -p leveled-audio-gui --example screenshots -- /path/to/workdir docs/images
scripts/make-icon.py 1024 assets/icon.png && scripts/make-icon.py 256 assets/icon-256.png
swift scripts/social-preview.swift          # macOS only: docs/images/social-preview.png
```

Bug reports and ideas are welcome as [issues](https://github.com/Marduk4all/leveled-audio/issues), questions in [Discussions](https://github.com/Marduk4all/leveled-audio/discussions). See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. The desktop app embeds fonts under their own licenses, see [licenses/](licenses/).
