#!/bin/bash
# Creates the standard (non-admin) test account `rdpspike` for the Phase 3
# multi-user work. Prompts for your admin password (sudo) and then for the new
# account's password. Keep the account until Phase 3 is verified.
set -euo pipefail

if dscl . -read /Users/rdpspike >/dev/null 2>&1; then
    echo "rdpspike already exists."
    exit 0
fi

echo "Creating rdpspike. You'll be asked for your admin password, then the new account's password."
sudo sysadminctl -addUser rdpspike -fullName "RDP Spike" -password -

dscl . -read /Users/rdpspike UniqueID NFSHomeDirectory
echo
echo "Done. Now log into rdpspike once (Apple menu > Lock Screen or Fast User Switching), then log out."
