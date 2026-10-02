#!/bin/bash
# Generate an MDM Privacy Preferences (PPPC) profile for Viga, for deployment
# through an MDM (Jamf, Kandji, Intune...). It pre-approves Accessibility and
# lets standard users approve Screen Recording themselves (macOS does not allow
# an MDM to grant Screen Recording outright).
#   packaging/make-pppc-profile.sh [Viga.app] > Viga-PPPC.mobileconfig
set -euo pipefail
APP="${1:-$(cd "$(dirname "$0")/.." && pwd)/target/Viga.app}"
ID=$(/usr/libexec/PlistBuddy -c "Print CFBundleIdentifier" "$APP/Contents/Info.plist")
REQ=$(codesign -dr - "$APP" 2>&1 | sed -n 's/^designated => //p' | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g')
[ -n "$REQ" ] || { echo "cannot read the designated requirement of $APP" >&2; exit 1; }
U1=$(uuidgen); U2=$(uuidgen)
entry() { cat <<E
        <dict><key>Identifier</key><string>$ID</string><key>IdentifierType</key><string>bundleID</string>
        <key>CodeRequirement</key><string>$REQ</string><key>$1</key><$2/></dict>
E
}
cat <<P
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>PayloadType</key><string>Configuration</string><key>PayloadVersion</key><integer>1</integer>
  <key>PayloadIdentifier</key><string>$ID.pppc</string><key>PayloadUUID</key><string>$U1</string>
  <key>PayloadDisplayName</key><string>Viga privacy permissions</string><key>PayloadScope</key><string>System</string>
  <key>PayloadContent</key><array><dict>
    <key>PayloadType</key><string>com.apple.TCC.configuration-profile-policy</string><key>PayloadVersion</key><integer>1</integer>
    <key>PayloadIdentifier</key><string>$ID.pppc.tcc</string><key>PayloadUUID</key><string>$U2</string>
    <key>Services</key><dict>
      <key>Accessibility</key><array>
$(entry Allowed true)
      </array>
      <key>ScreenCapture</key><array>
$(entry Authorization string | sed 's#<string/>#<string>AllowStandardUserToSetSystemService</string>#')
      </array>
    </dict>
  </dict></array>
</dict></plist>
P
