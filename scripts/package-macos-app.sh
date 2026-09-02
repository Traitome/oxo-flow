#!/usr/bin/env bash
# Package the desktop shell as a macOS .app bundle, plus .zip and .dmg.
# Single packaging path shared by CI (build-macos job) and `make bundle-macos`
# so a locally produced bundle is byte-comparable with the released one.
#
# Usage: scripts/package-macos-app.sh [target-triple] [version-label] [dist-dir]
#   target-triple   Rust target of an already-built desktop binary; empty
#                   means the host default (target/release).
#   version-label   String embedded in artifact names and CFBundleVersion
#                   (CI passes $RELEASE_TAG, e.g. v0.17.0; make passes v$VERSION).
#   dist-dir        Output directory for the .zip/.dmg (default: dist).
#
# Prerequisites: frontend built (crates/oxo-flow-web/static), desktop shell
# built (crates/oxo-flow-desktop/target[/triple]/release/oxo-flow-desktop),
# assets/oxo-flow.icns present. Run from the repository root.
set -euo pipefail

TARGET="${1:-}"
VERSION_LABEL="${2:?version-label required (e.g. v0.17.0)}"
DIST="${3:-dist}"

TARGET_DIR="target/release"
DESKTOP_DIR="crates/oxo-flow-desktop/target/release"
if [ -n "$TARGET" ]; then
  TARGET_DIR="target/${TARGET}/release"
  DESKTOP_DIR="crates/oxo-flow-desktop/target/${TARGET}/release"
fi

for required in \
  "${DESKTOP_DIR}/oxo-flow-desktop" \
  "crates/oxo-flow-web/static" \
  "assets/oxo-flow.icns"; do
  if [ ! -e "$required" ]; then
    echo "error: missing $required — build the frontend and the desktop shell first" >&2
    exit 1
  fi
done

# Hand-rolled .app bundle (cargo-bundle-style layout, extended with icon and
# code signature). The bundle executable is the wry-based desktop shell
# itself — a native window (WKWebView) around the embedded web server, no
# external browser involved:
# - Contents/MacOS/oxo-flow     desktop shell binary (serves the SPA on
#                               loopback and shows it in a native window;
#                               handles cwd=/ from Finder launches natively)
# - Contents/Resources/static   built SPA (served by the shell)
# - Contents/Resources/oxo-flow.icns
# - bundle-level ad-hoc code signature (arm64 macOS requires signed
#   executables; the linker's ad-hoc Mach-O signature is not a bundle
#   signature — unsigned bundles are reported as "damaged" by Gatekeeper on
#   Apple Silicon).
APP="oxo-flow.app"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"
cp "${DESKTOP_DIR}/oxo-flow-desktop" "${APP}/Contents/MacOS/oxo-flow"
cp -r crates/oxo-flow-web/static "${APP}/Contents/Resources/static"
cp assets/oxo-flow.icns "${APP}/Contents/Resources/oxo-flow.icns"
PLIST="${APP}/Contents/Info.plist"
printf '<?xml version="1.0" encoding="UTF-8"?>\n' > "$PLIST"
printf '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' >> "$PLIST"
printf '<plist version="1.0"><dict>\n' >> "$PLIST"
printf '  <key>CFBundleName</key><string>oxo-flow</string>\n' >> "$PLIST"
printf '  <key>CFBundleDisplayName</key><string>oxo-flow</string>\n' >> "$PLIST"
printf '  <key>CFBundleIdentifier</key><string>org.oxoflow.oxoflow</string>\n' >> "$PLIST"
printf '  <key>CFBundleVersion</key><string>%s</string>\n' "${VERSION_LABEL#v}" >> "$PLIST"
printf '  <key>CFBundleShortVersionString</key><string>%s</string>\n' "${VERSION_LABEL#v}" >> "$PLIST"
printf '  <key>CFBundlePackageType</key><string>APPL</string>\n' >> "$PLIST"
printf '  <key>CFBundleExecutable</key><string>oxo-flow</string>\n' >> "$PLIST"
printf '  <key>CFBundleIconFile</key><string>oxo-flow</string>\n' >> "$PLIST"
printf '  <key>LSMinimumSystemVersion</key><string>11.0</string>\n' >> "$PLIST"
printf '  <key>NSPrincipalClass</key><string>NSApplication</string>\n' >> "$PLIST"
printf '  <key>NSHighResolutionCapable</key><true/>\n' >> "$PLIST"
printf '</dict></plist>\n' >> "$PLIST"
# Ad-hoc (not Developer ID): makes the bundle internally consistent so
# Gatekeeper does not flag it as "damaged" once the download quarantine is
# cleared; a downloaded ad-hoc app is still blocked by Gatekeeper until the
# quarantine attribute is removed (see docs/guide/src/how-to/desktop-app.md).
codesign --force --deep --sign - "${APP}"
mkdir -p "$DIST"
(cd "$DIST" && zip -qry "oxo-flow-${VERSION_LABEL}-desktop-${TARGET:-host}-app.zip" "../${APP}")
if command -v hdiutil >/dev/null 2>&1; then
  hdiutil create -volname "oxo-flow" -srcfolder "${APP}" -ov -format UDZO \
    "${DIST}/oxo-flow-${VERSION_LABEL}-desktop-${TARGET:-host}.dmg"
else
  echo "hdiutil not found — produced ${DIST}/…-app.zip only (non-macOS host?)" >&2
fi
echo "→ ${DIST}/oxo-flow-${VERSION_LABEL}-desktop-${TARGET:-host}{-app.zip,.dmg}"
