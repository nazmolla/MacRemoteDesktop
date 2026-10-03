#!/bin/bash
# Install the Viga MULTI-USER stack: the root broker LaunchDaemon (public port)
# and the per-session Aqua LaunchAgent (one per logged-in user, loopback ports).
# This replaces the single-user per-user LaunchAgent (install-launchagent.sh);
# run `bootout` on that first if it's loaded, or it will also bind the port.
#
# Requires sudo. Env overrides: APP_DIR (default /Applications).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG_DIR="${PKG_DIR:-$REPO_ROOT/packaging}"
APP_DIR="${APP_DIR:-/Applications}"
PRODUCT="${PRODUCT:-Viga}"
PRODUCT_ID="${PRODUCT_ID:-viga}"
BUNDLE_PREFIX="${BUNDLE_PREFIX:-ca.nazmi}"
LABEL="${LABEL:-$BUNDLE_PREFIX.portico}"   # TCC-keyed bundle id; keep stable.

APP="$APP_DIR/$PRODUCT.app"
[ -d "$APP" ] || { echo "$PRODUCT.app not found at $APP — run make-app.sh first" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }

SUPPORT="/Library/Application Support/$PRODUCT"
mkdir -p "$SUPPORT"

# 1. Seed the root-owned policy + agent config (keep existing).
POLICY="$SUPPORT/policy.env"
if [ ! -f "$POLICY" ]; then cp "$PKG_DIR/policy.env.example" "$POLICY"; echo "==> seeded $POLICY"; fi
AGENT_ENV="$SUPPORT/agent.env"
if [ ! -f "$AGENT_ENV" ]; then
    # Agent settings shared by all sessions (BIND is overridden by --session-agent).
    printf 'ENABLE_H264=1\nADAPTIVE_BITRATE=1\n' > "$AGENT_ENV"
    echo "==> seeded $AGENT_ENV"
fi
chown root:wheel "$POLICY" "$AGENT_ENV"; chmod 644 "$POLICY" "$AGENT_ENV"

render() { # template dest
    sed -e "s#__LABEL__#$LABEL#g" -e "s#__APP_DIR__#$APP_DIR#g" \
        -e "s#__PRODUCT_ID__#$PRODUCT_ID#g" -e "s#__PRODUCT__#$PRODUCT#g" \
        "$1" > "$2"; chown root:wheel "$2"; chmod 644 "$2"; echo "==> wrote $2"
}

# 2. Broker LaunchDaemon (root, public port).
BROKER_PLIST="/Library/LaunchDaemons/$LABEL.broker.plist"
render "$PKG_DIR/broker.plist.template" "$BROKER_PLIST"
launchctl bootout system "$BROKER_PLIST" 2>/dev/null || true
launchctl bootstrap system "$BROKER_PLIST"
echo "==> broker loaded ($LABEL.broker)"

# 3. Session-agent LaunchAgent (system-wide; launchd starts one per Aqua session).
AGENT_PLIST="/Library/LaunchAgents/$LABEL.agent.plist"
render "$PKG_DIR/session-agent.plist.template" "$AGENT_PLIST"
# Bootstrap into every currently-logged-in GUI session.
for uid in $(ls /Library/Application\ Support 2>/dev/null >/dev/null; who | awk '/console/{print $1}' | sort -u | while read u; do id -u "$u" 2>/dev/null; done); do
    launchctl bootout "gui/$uid/$LABEL.agent" 2>/dev/null || true
    launchctl bootstrap "gui/$uid" "$AGENT_PLIST" 2>/dev/null \
        && echo "==> agent loaded in session uid=$uid" || true
done

cat <<EOF

Multi-user stack installed.
  Broker:   $BROKER_PLIST  (public 0.0.0.0:3389)
  Agent:    $AGENT_PLIST   (per-session, loopback 39000+uid%1000)
  Policy:   $POLICY
Edit $POLICY (or use the menu-bar app) to set PRIMARY_USER and MULTI_USER=1.
Ensure $PRODUCT.app has Screen Recording + Accessibility (System Settings or MDM PPPC).
EOF
