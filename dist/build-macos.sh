#!/bin/sh
# Build everything a macOS user needs, for both Intel and Apple Silicon.
#
#   ./dist/build-macos.sh            unsigned, for people who can use a terminal
#   SIGN_ID="Developer ID Application: Your Name (TEAMID)" \
#   NOTARY_PROFILE=rda ./dist/build-macos.sh    signed and notarised
#
# Without signing the result still works, but macOS will refuse to open it
# on the first try and the recipient has to go through System Settings. See
# DISTRIBUTING.md for what that costs and what to say to them.
set -eu

cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="$ROOT/dist/out"
rm -rf "$OUT"
mkdir -p "$OUT"

for t in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target list --installed | grep -qx "$t" || rustup target add "$t"
done

echo "==> command line tool"
cd "$ROOT/pipeline"
for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --target "$t"
done
lipo -create -output "$OUT/rda-ensemble" \
    "target/aarch64-apple-darwin/release/rda-ensemble" \
    "target/x86_64-apple-darwin/release/rda-ensemble"

echo "==> window"
cd "$ROOT/gui/src-tauri"
# The bundler's own dmg step drives Finder through AppleScript and fails
# without a desktop session, so the app is bundled here and the disk image
# is made below with hdiutil.
cargo tauri build --target universal-apple-darwin --bundles app
APP="target/universal-apple-darwin/release/bundle/macos/rda-ensemble.app"

if [ -n "${SIGN_ID:-}" ]; then
    echo "==> signing"
    # Hardened runtime is required for notarisation.
    codesign --force --deep --options runtime --timestamp \
        --sign "$SIGN_ID" "$APP"
    codesign --force --options runtime --timestamp \
        --sign "$SIGN_ID" "$OUT/rda-ensemble"
    codesign --verify --deep --strict --verbose=2 "$APP"
fi

cp -R "$APP" "$OUT/"
hdiutil create -volname "rda-ensemble" -srcfolder "$APP" \
    -ov -format UDZO "$OUT/rda-ensemble.dmg" >/dev/null

if [ -n "${NOTARY_PROFILE:-}" ]; then
    echo "==> notarising (a few minutes)"
    xcrun notarytool submit "$OUT/rda-ensemble.dmg" \
        --keychain-profile "$NOTARY_PROFILE" --wait
    # Stapling puts the ticket in the file, so it opens even offline.
    xcrun stapler staple "$OUT/rda-ensemble.dmg"
    xcrun stapler staple "$OUT/rda-ensemble.app"
    hdiutil create -volname "rda-ensemble" -srcfolder "$OUT/rda-ensemble.app" \
        -ov -format UDZO "$OUT/rda-ensemble.dmg" >/dev/null
fi

cd "$ROOT"
cp pipeline/README.md "$OUT/command-line.md"
cp gui/README.md "$OUT/window.md"

echo
echo "==> $OUT"
ls -lh "$OUT" | tail -n +2 | awk '{printf "    %-28s %s\n", $9, $5}'
if [ -z "${SIGN_ID:-}" ]; then
    echo
    echo "    Unsigned. Whoever downloads this will be told the developer"
    echo "    cannot be verified; DISTRIBUTING.md says what to tell them."
fi
