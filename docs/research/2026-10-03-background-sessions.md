# Simultaneous user sessions: findings and the remaining route (2026-10-03)

## What blocks Viga's own background session
Tested in the macOS 27 VM (`spikes/skylight/slvd.m`, `spikes/multisession/`):

- A user's own agent can create a virtual display (`CGVirtualDisplay` or SkyLight's
  `SLVirtualDisplay`) only while that user's session is on the console. The moment the
  session goes to the background (Fast User Switching), creation fails with
  CoreGraphics error 1000. WindowServer logs:
  `Virtual display is not allowed in off-console session`.
- A root agent in an off-console login-window session fails the same way.
- Apple's `ScreensharingAgent` creates displays in background sessions because it holds the
  private entitlement `com.apple.private.SkyLight.virtualdisplay`, which third parties
  cannot obtain.
- Bypassing the check in WindowServer is not an option for a product (and is out of scope).

## The route that remains: Apple's Screen Sharing server as the session host
Apple's Screen Sharing ("log in as yourself") creates a background session for a second
user and drives its display with Apple's entitled agent. Third-party clients already use
it: Edovia Screens connects to the built-in Screen Sharing server and offers
"Log in as yourself", which "starts a background session with your own desktop"
(help.edovia.com, Screens 5, Sharing Mac session).

Design for Viga:
- User A (console or first remote user): unchanged, Viga's own capture path.
- User B connecting while A's session exists: Viga authenticates B over NLA, then acts as a
  local client of the Screen Sharing server on 127.0.0.1:5900 for B, choosing
  "log in as yourself". Viga re-encodes B's framebuffer to H.264/EGFX and forwards input
  and clipboard over that local connection.
- Costs: B's picture comes through standard (not High Performance) Screen Sharing on
  loopback, so resolution/HiDPI and audio are limited to what that protocol offers;
  Screen Sharing must be enabled; B needs Screen Sharing access.
- Licence: Apple's macOS licence terms on terminal-services-style use need legal review
  before sale (spec §5.3).

## Next steps
1. Manual check in the VM with Apple's Screen Sharing app: sign in as `vmspike` while admin
   holds the console, choose "Log in as yourself", confirm a background session renders,
   and note its resolution and whether the display can be resized.
2. If it renders: decide how the local client authenticates B (to be designed carefully;
   it reuses credentials B already proved over NLA).
