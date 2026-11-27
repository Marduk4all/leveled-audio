#!/usr/bin/env bash
# Baut aus der GUI ein macOS-Programm „Leveled Audio.app“ (mit Icon), das sich per
# Doppelklick (Finder, Dock, Launchpad) ohne Terminal-Fenster starten lässt.
#
# Aufruf:  scripts/macos-app.sh [GUI-Binary] [Zielordner]
#   Default: target/release/leveled-audio-gui  →  target/release/Leveled Audio.app
#   Vorher bauen: cargo build --release -p leveled-audio-gui
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${1:-$root/target/release/leveled-audio-gui}"
out="${2:-$(dirname "$bin")}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

[ -x "$bin" ] || { echo "GUI-Binary fehlt: $bin (erst: cargo build --release -p leveled-audio-gui)" >&2; exit 1; }

name="Leveled Audio"
app="$out/$name.app"
rm -rf "$app" "$out/leveled-audio.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/leveled-audio-gui"
chmod +x "$app/Contents/MacOS/leveled-audio-gui"
cp -r "$root/licenses" "$app/Contents/Resources/"

# Icon: assets/icon.png (1024 px, aus scripts/make-icon.py) → AppIcon.icns
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s "$root/assets/icon.png" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$root/assets/icon.png" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$(dirname "$iconset")"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleDisplayName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>org.leveledaudio.gui</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleExecutable</key><string>leveled-audio-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ohne Apple-Entwicklerzertifikat: Ad-hoc-Signatur, damit macOS das Bundle
# als Einheit akzeptiert (Apple Silicon verlangt eine Signatur).
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || echo "Hinweis: codesign nicht ausgeführt" >&2

echo "$app"
