#!/bin/bash
# Lets your account run Phase 3 spike tools AS THE TEST USER `rdpspike`, inside
# rdpspike's login session, without a password prompt. It does NOT allow running
# anything as root: the only root-owned piece is the fixed wrapper below, which
# copies a tool from this repo's spikes/multisession/probe/ and runs it as rdpspike.
#
#   setup-spike-sudo.sh            install
#   setup-spike-sudo.sh --remove   remove everything this installed
set -euo pipefail

DIR=/usr/local/libexec/viga-spike
WRAPPER="$DIR/run-as-spike"
SUDOERS=/etc/sudoers.d/viga-spike
ME="$(id -un)"
REPO_PROBE="$(cd "$(dirname "$0")" && pwd)/probe"

if [ "${1:-}" = "--remove" ]; then
    sudo rm -f "$SUDOERS"
    sudo rm -rf "$DIR"
    echo "Removed $SUDOERS and $DIR."
    exit 0
fi

dscl . -read /Users/rdpspike >/dev/null 2>&1 || { echo "rdpspike does not exist; run create-rdpspike.sh first."; exit 1; }

TMP="$(mktemp)"
cat > "$TMP" <<EOF
#!/bin/bash
# Installed by setup-spike-sudo.sh. Runs a spike tool as rdpspike in rdpspike's session.
set -euo pipefail
name="\${1:-}"; shift || true
[[ "\$name" =~ ^[a-z0-9-]+\$ ]] || { echo "usage: run-as-spike <tool-name> [args]" >&2; exit 2; }
src="$REPO_PROBE/\$name"
[ -f "\$src" ] || { echo "no such tool: \$src" >&2; exit 2; }
dst="$DIR/bin/\$name"
/usr/bin/install -o root -g wheel -m 755 "\$src" "\$dst"
uid="\$(/usr/bin/id -u rdpspike)"
exec /bin/launchctl asuser "\$uid" /usr/bin/sudo -u rdpspike "\$dst" "\$@"
EOF

echo "Installing $WRAPPER (root-owned) and $SUDOERS. You'll be asked for your admin password."
sudo /bin/mkdir -p "$DIR/bin"
sudo /usr/sbin/chown -R root:wheel "$DIR"
sudo /bin/chmod 755 "$DIR" "$DIR/bin"
sudo /usr/bin/install -o root -g wheel -m 755 "$TMP" "$WRAPPER"
rm -f "$TMP"

RULE="$ME ALL=(root) NOPASSWD: $WRAPPER"
echo "$RULE" > /tmp/viga-spike.sudoers.$$
if sudo /usr/sbin/visudo -cf /tmp/viga-spike.sudoers.$$ >/dev/null; then
    sudo /usr/bin/install -o root -g wheel -m 440 /tmp/viga-spike.sudoers.$$ "$SUDOERS"
else
    echo "sudoers rule failed validation; nothing installed." ; rm -f /tmp/viga-spike.sudoers.$$; exit 1
fi
rm -f /tmp/viga-spike.sudoers.$$

echo "Installed. Rule: $RULE"
echo "Remove later with: $0 --remove"
