#!/bin/sh
# Construit dist/Snappy.app (release) : app sans icône dans le Dock (LSUIElement),
# pilotée depuis la barre des menus.
#
# Usage : sh tools/bundle_macos.sh
#
# Attention : l'autorisation « Enregistrement de l'écran » de macOS est liée à la
# signature de l'app. Avec la signature ad hoc ci-dessous, l'identité change à chaque
# recompilation : macOS peut redemander l'autorisation après chaque build (au besoin,
# retirer puis rajouter Snappy dans Réglages Système > Confidentialité et sécurité).
# Un certificat de développeur stable éviterait ces demandes.
set -eu

cd "$(dirname "$0")/.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

cargo build --release

APP=dist/Snappy.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/snappy "$APP/Contents/MacOS/snappy"
cp assets/icon.icns "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Snappy</string>
  <key>CFBundleDisplayName</key><string>Snappy</string>
  <key>CFBundleIdentifier</key><string>com.snappy.app</string>
  <key>CFBundleExecutable</key><string>snappy</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Snappy enregistre le micro dans vos vidéos d'écran quand l'option Micro est activée.</string>
</dict>
</plist>
PLIST

# Signature ad hoc : nécessaire pour que macOS accepte de lancer le bundle (voir la
# remarque en tête sur l'autorisation d'enregistrement de l'écran).
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "codesign indisponible : app non signée"

du -sh "$APP"
echo "OK : $APP"
