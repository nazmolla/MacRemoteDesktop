# macOS type check on Linux

`check.sh` runs `cargo check` (or `cargo clippy` with `MODE=clippy`) for
`aarch64-apple-darwin` on a Linux machine. It catches compile errors and lint
failures in macOS-only code without a Mac.

How it works:

- `cc/cc`, `cc/c++`, `cc/ar` (deliberately not on `PATH`, so host build
  scripts still link with the real toolchain) replace the target C toolchain and only create empty
  output files. Build scripts succeed; nothing is linked (check never links).
- `bin/swift`, `bin/xcrun`, `bin/xcode-select` satisfy the ScreenCaptureKit
  crate's build script.
- `sdk/usr/include` is a stub SDK with the few declarations BoringSSL's public
  headers need, so `boring-sys` can run bindgen.

It reproduces the macOS CI results for this crate (for example the dead-code
errors the `audit log (macos integration)` job reports). It cannot run tests or
catch link errors, so a real Mac build is still required before release.
