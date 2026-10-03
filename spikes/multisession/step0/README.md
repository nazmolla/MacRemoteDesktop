# Step-0 proof: native-framebuffer capture in a BACKGROUND session

Gates the whole Phase-3 broker (`docs/superpowers/plans/2026-10-03-phase-3-broker.md`).
Proves the one capability never actually tested: an in-session Aqua LaunchAgent capturing
*its own background session's real framebuffer* and injecting input — Jump Desktop's model,
no SkyLight entitlement. Run ONLY in the VM (`[[virtual-display-churn-crashes-windowserver]]`).

## Why this differs from the failed 2026-10-02 run
That run launched the probe via `launchctl asuser <uid>` from root, which bound SCK to the
**console**, so it captured admin's desktop and input landed on the console. An `Aqua`
LaunchAgent is started by launchd *inside each GUI session itself* — console and background
— so SCK sees that session's own displays. No virtual display is created (that is the only
entitlement-gated part, and Jump can't do it either), so this exercises the plain
mirror-primary capture path.

## Procedure (VM: `spikes/vm/vmhost ~/VMs/viga-test --gui`)
1. Boot the VM; `admin` is on the console. Install Viga there (the VM-verified `.pkg`), so
   `/Applications/Viga.app` exists and holds Screen Recording + Accessibility (system TCC,
   already granted in the VM per the 2026-10-03 note).
2. **Log `vmspike` in via Fast User Switching at the VM console** (menu-bar → Login Window →
   sign in as vmspike). This is the only manual/console action. It creates vmspike's
   **background** session (admin keeps the console). Password: VM `/var/root/vmspike-pw`.
3. As root in the VM: `mkdir -p /Users/Shared/vigatest && cp config.env
   /Users/Shared/vigatest/step0.config.env && cp ca.nazmi.viga.step0.plist
   /Library/LaunchAgents/` then `chown root:wheel /Library/LaunchAgents/ca.nazmi.viga.step0.plist`.
   launchd auto-loads it into vmspike's (and admin's) Aqua session on next load; force
   vmspike's copy: `launchctl bootstrap gui/$(id -u vmspike) /Library/LaunchAgents/ca.nazmi.viga.step0.plist`.
4. From the **host**, RDP to the VM at `:3392` (sdl-freerdp). Two agent copies are
   listening (admin + vmspike) — connect to the one in vmspike's session (distinguish by
   binding per-uid in a follow-up; for the proof, confirm the picture is **vmspike's empty
   desktop**, not admin's). Move the mouse / type; confirm input lands in vmspike's session
   while admin's console is untouched.

## Pass / fail
- **PASS:** the RDP client shows vmspike's background-session desktop and input injects there
  → simultaneous multi-user is proven; build the broker (plan steps 2-6).
- **FAIL (SCK captures console / black / TCC declined):** background SCK needs something the
  console path doesn't → fall back to the Screen-Sharing-delegation route (Edovia's model,
  `docs/research/2026-10-03-background-sessions.md`) for non-console users.

Check `/Users/Shared/vigatest/step0.{out,err}.log` and
`grep -i "bound_display\|console\|capture" ` for which display SCK bound to.
Remove the LaunchAgent and shut the VM down after (`launchctl bootout`, `rm` the plist).
