#!/bin/bash
# Install + (re)load the Viga LaunchAgent for the current user.
#
# Seeds ~/Library/Application Support/macrdp/config.env from the example on
# first run, renders the LaunchAgent plist from the template, and bootstraps it.
#
# Env overrides:
#   APP_DIR=/Applications     # where macrdp.app was installed (default /Applications)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG_DIR="${PKG_DIR:-$REPO_ROOT/packaging}"
APP_DIR="${APP_DIR:-/Applications}"
# MUST match the BUNDLE_PREFIX used by make-app.sh (and gui/make-tray-app.sh),
# or the controller targets a different label than the agent installed here.
# Product name and lower-case id; keep in step with src/brand.rs and make-app.sh.
PRODUCT="${PRODUCT:-Viga}"
PRODUCT_ID="${PRODUCT_ID:-viga}"
BUNDLE_PREFIX="${BUNDLE_PREFIX:-ca.nazmi}"
# Same id as the app bundle (see make-app.sh), kept from the Portico name.
LABEL="${LABEL:-$BUNDLE_PREFIX.portico}"
UID_NUM="$(id -u)"

APP="$APP_DIR/$PRODUCT.app"
[ -d "$APP" ] || { echo "$PRODUCT.app not found at $APP — run packaging/make-app.sh first" >&2; exit 1; }

# 1. Seed config.env if absent.
SUPPORT="$HOME/Library/Application Support/$PRODUCT"
mkdir -p "$SUPPORT" "$HOME/Library/Logs" "$HOME/Library/LaunchAgents"
CONFIG="$SUPPORT/config.env"
if [ ! -f "$CONFIG" ]; then
    cp "$PKG_DIR/config.env.example" "$CONFIG"
    echo "==> seeded $CONFIG (edit to taste)"
else
    echo "==> keeping existing $CONFIG"
fi

# 2. Render the LaunchAgent plist from the template.
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
sed -e "s#__LABEL__#$LABEL#g" -e "s#__APP_DIR__#$APP_DIR#g" -e "s#__HOME__#$HOME#g" \
    -e "s#__PRODUCT_ID__#$PRODUCT_ID#g" -e "s#__PRODUCT__#$PRODUCT#g" \
    "$PKG_DIR/launchagent.plist.template" > "$PLIST"
echo "==> wrote $PLIST"

# 3. (Re)bootstrap the agent. `bootstrap` immediately after `bootout` can fail
#    with "Input/output error" (EIO, 5) while launchd is still tearing the old
#    job down — a race that otherwise leaves the agent UNloaded (server doesn't
#    come back). Retry a few times; the final attempt runs without suppressing
#    stderr so a genuine failure is surfaced (and aborts via set -e).
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
bootstrapped=0
for _ in 1 2 3 4 5; do
    if launchctl bootstrap "gui/$UID_NUM" "$PLIST" 2>/dev/null; then
        bootstrapped=1
        break
    fi
    sleep 1
done
[ "$bootstrapped" = 1 ] || launchctl bootstrap "gui/$UID_NUM" "$PLIST"
launchctl enable "gui/$UID_NUM/$LABEL"
launchctl kickstart -k "gui/$UID_NUM/$LABEL"

echo
echo "Loaded $LABEL."
echo "  status:  launchctl print gui/$UID_NUM/$LABEL | grep -E 'state|pid'"
echo "  logs:    tail -f ~/Library/Logs/$PRODUCT_ID.log"
echo "  apply config change:  launchctl kickstart -k gui/$UID_NUM/$LABEL"
echo "  stop:    launchctl bootout gui/$UID_NUM/$LABEL"
