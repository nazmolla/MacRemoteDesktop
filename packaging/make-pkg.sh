#!/bin/bash
# Build a signed installer package for Viga: installs Viga.app into
# /Applications and sets up the LaunchAgent for the console user.
#   packaging/make-pkg.sh            (run make-app.sh first; uses target/Viga.app)
#   NOTARY_PROFILE=<profile> packaging/notarize.sh target/Viga-<version>.pkg
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$REPO_ROOT/target/Viga.app"
[ -d "$APP" ] || { echo "build the app first: packaging/make-app.sh" >&2; exit 1; }
VERSION=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
INSTALLER_ID="${INSTALLER_IDENTITY:-$(security find-identity -v | awk -F'"' '/Developer ID Installer/{print $2; exit}')}"
[ -n "$INSTALLER_ID" ] || { echo "no Developer ID Installer identity in the keychain" >&2; exit 1; }
ROOT="$(mktemp -d)"; trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/Applications"
/usr/bin/ditto "$APP" "$ROOT/Applications/Viga.app"
OUT="$REPO_ROOT/target/Viga-$VERSION.pkg"
/usr/bin/pkgbuild --root "$ROOT" --install-location / \
    --scripts "$REPO_ROOT/packaging/pkg-scripts" \
    --identifier ca.nazmi.viga.pkg --version "$VERSION" \
    --sign "$INSTALLER_ID" --timestamp "$OUT"
/usr/sbin/pkgutil --check-signature "$OUT" | head -3
echo "==> $OUT"
