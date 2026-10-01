# Virtual display stability

Date: 2026-09-30 to 2026-10-01. Machine: Mac mini (Apple M6), macOS 27.
Spike: `spikes/displayhost/` (a one-display-per-process host driven over stdin,
plus `ctl.py`, a controller that issues random re-modes).

## Question

Why did virtual display re-modes "stick" (the display stayed at an old size),
and what does a reliable implementation look like?

## Findings

1. **Explicit mode selection freezes the display.** After
   `CGConfigureDisplayWithDisplayMode` on a virtual display (any scope, including
   `kCGConfigureForAppOnly`), later `applySettings:` calls publish no new modes
   for the life of the process. Every "stuck" re-mode traced back to an earlier
   explicit selection, including the one made right after creation.
2. **`CGRestorePermanentDisplayConfiguration` is not a fix.** It resets every
   display's configuration; used before each re-apply it made about 30% of
   activations fail and rarely un-froze the display.
3. **Without explicit selection, 1× re-modes are reliable.** Registering one
   mode with `hiDPI=0` makes it current by itself. In the spike, 100 of 100
   random re-modes succeeded with a selection-free design (81 in place,
   19 through a host restart).
4. **Retina (`hiDPI=1`) is decided by an opaque WindowServer heuristic.**
   macOS publishes the Retina mode and its 1× twin and picks one. The choice
   depends on aspect ratio, point size and the declared physical size
   (`sizeInMillimeters`), and macOS keeps the 1× twin when the previous 1× mode
   had the same point size. A request that lands on the 1× twin can leave the
   display stuck: a following 1× registration was ignored, even when retried.
5. **One virtual display per process.** A second display created in the same
   process (even sequentially, after the first was released) does not come
   online. Tests that create displays must each run in their own process.
6. **Registering `CGDisplayRegisterReconfigurationCallback` breaks creation.**
   With the callback registered, the process's own virtual display never came
   online. Poll the display state instead.
7. **A re-created identity must wait for the old one to leave.** Creating a
   display with the vendor/product/serial of one that is still being torn down
   is rejected (`initWithDescriptor:` returns nil).
8. **Display churn overloads the whole machine.** Every display change makes
   WindowServer, ColorSync and every running app re-process the configuration.
   Several overlapping changes per second crashed WindowServer twice (SIGABRT in
   `SkyLight WS::Displays::GenerateModeListForDisplay` during deferred hotplug
   processing), which logs out every session. All sessions share one
   WindowServer, so a second user account does not isolate the crash. A
   serialized but continuous stream of changes wedged the machine until a hard
   restart. Plugging monitors or casting is fine because it happens at human
   pace, one change at a time.

## What the product does (F5 in FORK.md)

- Registers a single mode and waits up to 3 s for it to become current. Never
  selects a mode explicitly; never calls `CGRestorePermanentDisplayConfiguration`.
- A 1× change is re-applied once if it does not land.
- At most one display change per second per process (`MIN_CHANGE_GAP`), on top
  of the 400 ms resize debounce in `capture.rs`.
- High-DPI clients get 1× at their exact pixel size by default. Retina modes are
  tried only with `MACRDP_RETINA_VD=1`, and fall back to 1× when they do not land.
- The display is created at the client's pixel size, never at the point size of
  a later Retina request.

## Verification

On-device tests in `src/virtual_display/mod.rs` (`planned_tests`), each run in
its own process (see the module comment): sRGB colorspace, 1× plan, Retina plan,
fallback, and a six-step resize sequence. All pass with the defaults.

## Open

- Reliable Retina: needs either a way to steer the default (for example a
  declared physical size chosen per session) or an architecture in which a
  display can be replaced without changing what the rest of the server uses.
  Test only at realistic pace.
