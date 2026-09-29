# Phase 0 — Foundation + Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn this repo into a working fork of `clintcan/macrdp` with fork bookkeeping, budget-friendly CI, a color-fidelity harness, a host/client performance harness with recorded baselines (including Jump Desktop), a throwaway spike that finds the macOS mechanism for simultaneous multi-user sessions, and submitted Apple entitlement requests.

**Architecture:** Upstream macrdp is merged in with full history so future upstream syncs are plain merges. New test tooling lives in test-only Rust modules (`#[cfg(test)]`), a Python sampler under `scripts/perf/`, a Swift workload generator under `tools/workload/`, and a PowerShell client sampler for Windows. The spike lives in `spikes/multisession/` and is explicitly throwaway; its output is a written verdict.

**Tech Stack:** Rust (stable, per `rust-toolchain.toml`), VideoToolbox (via macrdp's `src/videotoolbox.rs`), ffmpeg/ffprobe (decode only), FreeRDP `sdl-freerdp` (loopback client), Python 3 (Xcode CLT), Swift 5 mode + AppKit, PowerShell 5+ (Windows client side), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md` (§3 approach, §5.3 spike risks, §8 color, §9 performance, §12 testing, §13 productization, §14 Phase 0 row).

## Global Constraints

- Host dev machine: macOS 27 on Apple M6; Homebrew at `/opt/homebrew`.
- Canonical upstream: `https://github.com/clintcan/macrdp` (branch `main`). Our remote: `https://github.com/nazmolla/MacRemoteDesktop` (private).
- License: keep upstream `MIT OR Apache-2.0`; no GPL dependencies (`cargo deny` via existing `deny.toml`).
- Binary/crate name stays `macrdp` in Phase 0 (product name is chosen in Phase 5; product name must not contain Microsoft/Apple trademarks).
- Every deliberate difference from upstream code is recorded in `FORK.md` (divergence log).
- Color target (spec §1.3/§8): static content bit-exact after refinement; moving content ΔE2000 < 1 with AVC444. Phase 0 only **measures** the current AVC420 baseline; it does not gate on these.
- Performance budgets (spec §9): broker idle ~0% / < 20 MB; session agent idle < 1% of one core / < 150 MB; active 4K@60 < 15% of one performance core / < 300 MB. Phase 0 only **records** baselines.
- Private repo CI minutes: macOS runners bill at 10×; macOS jobs must not run on every push.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Local agents (`local-delegate` MCP) may do mechanical steps (doc text, boilerplate); every result is reviewed by diff before commit.

## Review Focus

1. **ffmpeg decodes to an unexpected pixel format** (e.g. `yuv420p10le`, or a converted range) → the harness must refuse to measure rather than report a wrong ΔE. Pinned by the `parse_probe` rejection test in Task 5.
2. **Frame sizes that are not multiples of 16** (the windowed-client case, e.g. 1713×1288) → metrics must crop to the source size and still measure correctly. Pinned by the odd-size cases in Tasks 4 and 5.
3. **The measured server process dies mid-run** → the sampler must exit non-zero and say so, never write zeros that look like a great result. Pinned by `test_sample_exits_when_process_dies` in Task 6.
4. **Upstream merge touches files we also own** (`README.md`, `CLAUDE.md`, `.gitignore`, `.github/workflows/*`) → our additions must survive and upstream content must stay intact; build and tests must pass after merge. Pinned by the post-merge build/test step in Task 2 and the CI check in Task 3.
5. **Loopback perf run where the client window is itself captured** (infinite mirror inflating "idle" CPU) → the harness must capture a virtual display the client window is not on. Pinned by the idle-CPU sanity check in Task 8.

---

### Task 1: Developer toolchain

**Files:** none (machine setup).

**Interfaces:**
- Produces: `cargo`, `rustc`, `sdl-freerdp`, `ffmpeg`, `ffprobe`, `python3`, `swiftc` on `PATH`.

- [ ] **Step 1: Check what is missing**

Run: `for t in cargo rustc sdl-freerdp ffmpeg ffprobe python3 swiftc gh; do printf '%-12s ' $t; command -v $t || echo MISSING; done`
Expected today: `cargo`, `rustc`, `sdl-freerdp` MISSING; others present.

- [ ] **Step 2: Install Rust via rustup and FreeRDP via Homebrew**

```bash
brew install rustup freerdp
rustup-init -y --no-modify-path --default-toolchain stable
source "$HOME/.cargo/env"
```

- [ ] **Step 3: Verify**

Run: `source "$HOME/.cargo/env"; cargo --version && rustc --version && sdl-freerdp --version 2>&1 | head -1 && ffprobe -version | head -1`
Expected: four version lines, no "not found".

(No commit — nothing in the repo changed.)

---

### Task 2: Import upstream macrdp with full history

**Files:**
- Modify (merge result): whole tree gains upstream files; our `docs/research/**` and `docs/superpowers/**` stay.
- Create: `docs/research/phase0-baseline.md`

**Interfaces:**
- Consumes: Task 1 toolchain.
- Produces: git remote `upstream` → `clintcan/macrdp`; a building tree; `docs/research/phase0-baseline.md` with a "Build/Test" section later tasks append to.

- [ ] **Step 1: Add upstream and fetch**

```bash
cd ~/Code/MacRemoteDesktop
git remote add upstream https://github.com/clintcan/macrdp.git
git fetch upstream --tags
git log --oneline -1 upstream/main
```
Expected: one commit line (newer than `b65eb08`).

- [ ] **Step 2: Merge upstream history**

```bash
git merge upstream/main --allow-unrelated-histories -m "merge: import clintcan/macrdp upstream history

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
Expected: merge succeeds with no conflicts (our files are only under `docs/research/` and `docs/superpowers/`, which upstream does not have). If a conflict appears, keep both sides' content and re-run `git status` until clean.

- [ ] **Step 3: Build and test the untouched upstream code**

```bash
source "$HOME/.cargo/env"
cargo build --locked 2>&1 | tail -3
cargo test --locked 2>&1 | grep -E '^test result:' 
```
Expected: build finishes; every `test result:` line says `0 failed`. Record the total passed count.

- [ ] **Step 4: Record the baseline**

Create `docs/research/phase0-baseline.md`:

```markdown
# Phase 0 baselines

## Build/Test (upstream at import)
- Upstream commit: <output of `git rev-parse --short upstream/main`>
- Toolchain: <output of `rustc --version`>
- `cargo test --locked`: <N> passed, 0 failed (macOS 27, Apple M6)
```
Replace each `<…>` with the actual command output before saving.

- [ ] **Step 5: Commit and push**

```bash
git add docs/research/phase0-baseline.md
git commit -m "docs: record upstream import build/test baseline

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 3: Fork bookkeeping and CI budget

**Files:**
- Create: `NOTICE`, `FORK.md`
- Modify: `README.md` (prepend fork banner), `CLAUDE.md` (prepend fork section), `.github/workflows/ci.yml` (`on:` block, macOS jobs gating), `.github/workflows/release.yml` (`on:` block), `.github/workflows/security.yml` (cron)

**Interfaces:**
- Produces: `FORK.md` with a `## Divergence log` table later tasks append rows to, format `| # | File | Change | Reason | Upstreamable? |`.

- [ ] **Step 1: Write `NOTICE`**

```text
MacRemoteDesktop (working name)
Copyright (c) 2026 nazmolla

This product is a fork of macrdp (https://github.com/clintcan/macrdp),
Copyright (c) the macrdp contributors, licensed under MIT OR Apache-2.0.

It includes vendored and modified crates from IronRDP
(https://github.com/Devolutions/IronRDP), Copyright (c) Devolutions Inc.,
licensed under MIT OR Apache-2.0.

Portions of src/avc444.rs are ported from FreeRDP
(https://github.com/FreeRDP/FreeRDP), licensed under Apache-2.0.
```

- [ ] **Step 2: Write `FORK.md`**

```markdown
# Fork notes

This repository is a fork of [clintcan/macrdp](https://github.com/clintcan/macrdp).
Design: `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md`.

## Syncing upstream
    git fetch upstream --tags
    git merge upstream/main
    cargo build --locked && cargo test --locked
Resolve conflicts in favor of upstream unless the file appears in the divergence log below.

## Divergence log
| # | File | Change | Reason | Upstreamable? |
|---|------|--------|--------|---------------|
| F1 | `.github/workflows/*.yml` | macOS jobs on PR/dispatch only; release on dispatch only; security weekly | Private-repo CI minutes (macOS bills 10×) | No |
```

- [ ] **Step 3: Prepend the README banner**

Insert at the very top of `README.md`:

```markdown
> **Fork notice.** This is a private fork of [macrdp](https://github.com/clintcan/macrdp) being restructured into a negotiated, headless-first RDP server for macOS. See [FORK.md](FORK.md) and the [design spec](docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md). Upstream README follows unchanged.

---

```

- [ ] **Step 4: Prepend the CLAUDE.md fork section**

Insert at the very top of `CLAUDE.md`:

```markdown
# Fork context (read first)

- This repo is a fork of clintcan/macrdp. The target architecture is in `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md`; the active plan is under `docs/superpowers/plans/`.
- Record every deliberate difference from upstream code in `FORK.md` → Divergence log.
- Guiding rule: sessions are configured by negotiation with the client, not flags. New code must not add user-facing flags.
- Performance and color accuracy are hard requirements (spec §8, §9).
- Upstream's own guidance follows unchanged.

---

```

- [ ] **Step 5: Restrict CI triggers**

In `.github/workflows/ci.yml` replace the `on:` block with:

```yaml
on:
  push:
    branches:
      - main
  pull_request:
  workflow_dispatch:
```

and add this line to **each macOS job** (the jobs with `runs-on: macos-14`), directly under `runs-on:`:

```yaml
    if: github.event_name != 'push'
```

In `.github/workflows/release.yml` replace the `on:` block with:

```yaml
on:
  workflow_dispatch:
    inputs:
      tag:
        description: 'Existing tag to (re)build, e.g. v0.5.1'
        required: true
```

In `.github/workflows/security.yml` change the cron line to weekly:

```yaml
    - cron: '12 7 * * 1'
```

- [ ] **Step 6: Validate workflow YAML locally**

Run: `for f in .github/workflows/*.yml; do python3 -c "import sys,yaml; yaml.safe_load(open('$f'))" 2>/dev/null && echo "ok $f" || ruby -ryaml -e "YAML.load_file('$f'); puts 'ok $f'"; done`
Expected: `ok` for all three files.

- [ ] **Step 7: Commit, push, and confirm CI**

```bash
git add NOTICE FORK.md README.md CLAUDE.md .github/workflows
git commit -m "chore(fork): NOTICE, FORK.md divergence log, fork banners, CI budget triggers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
sleep 20; gh run list --limit 3
```
Expected: a `ci` run for this push whose macOS jobs show as skipped and Linux job queued/in progress. Then `gh run watch` the run; expected: Linux job passes.

---

### Task 4: Color metrics and test pattern (test-only modules)

**Files:**
- Create: `src/color_metrics.rs`, `src/color_pattern.rs`
- Modify: `src/main.rs` (module declarations next to `mod conn_test;`)

**Interfaces:**
- Produces:
  - `color_metrics::srgb8_to_lab(rgb: [u8; 3]) -> [f64; 3]`
  - `color_metrics::delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64`
  - `color_metrics::yuv709_full_to_rgb8(y: u8, cb: u8, cr: u8) -> [u8; 3]`
  - `color_metrics::rgb8_to_yuv709_full(rgb: [u8; 3]) -> [u8; 3]`
  - `color_pattern::Rect { x, y, w, h: usize }`, `color_pattern::Patch { rect: Rect, srgb: [u8; 3] }`
  - `color_pattern::Pattern { width, height: usize, bgra: Vec<u8>, patches: Vec<Patch>, edge: Rect }`
  - `color_pattern::generate(width: usize, height: usize) -> Pattern`
  - `color_pattern::COLORCHECKER_SRGB: [[u8; 3]; 24]`

- [ ] **Step 1: Declare the modules**

In `src/main.rs`, directly after the existing lines
```rust
#[cfg(test)]
mod conn_test;
```
add
```rust
#[cfg(test)]
mod color_metrics;
#[cfg(test)]
mod color_pattern;
```

- [ ] **Step 2: Write the failing metric tests**

Create `src/color_metrics.rs` containing only the tests for now:

```rust
//! Color-fidelity metrics for the test harness: sRGB → CIELAB (D65),
//! CIEDE2000, and full-range BT.709 YUV ↔ RGB (the interpretation mstsc
//! applies to AVC420 luma/chroma). Test-only.

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn lab_of_white_black_red() {
        let w = srgb8_to_lab([255, 255, 255]);
        assert!(close(w[0], 100.0, 0.01) && close(w[1], 0.0, 0.01) && close(w[2], 0.0, 0.01), "{w:?}");
        let k = srgb8_to_lab([0, 0, 0]);
        assert!(close(k[0], 0.0, 0.01), "{k:?}");
        let r = srgb8_to_lab([255, 0, 0]);
        assert!(close(r[0], 53.24, 0.05) && close(r[1], 80.09, 0.05) && close(r[2], 67.20, 0.05), "{r:?}");
    }

    // Reference pairs from Sharma, Wu & Dalal (2005), "The CIEDE2000
    // Color-Difference Formula: Implementation Notes", Table 1.
    #[test]
    fn ciede2000_matches_sharma_reference_pairs() {
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        ];
        for (a, b, want) in cases {
            let got = delta_e_2000(a, b);
            assert!(close(got, want, 1e-4), "{a:?} vs {b:?}: got {got}, want {want}");
            assert!(close(delta_e_2000(b, a), want, 1e-4), "not symmetric for {a:?}");
        }
        assert_eq!(delta_e_2000([42.0, 10.0, -5.0], [42.0, 10.0, -5.0]), 0.0);
    }

    #[test]
    fn yuv709_full_range_endpoints() {
        assert_eq!(yuv709_full_to_rgb8(255, 128, 128), [255, 255, 255]);
        assert_eq!(yuv709_full_to_rgb8(0, 128, 128), [0, 0, 0]);
        assert_eq!(yuv709_full_to_rgb8(128, 128, 128), [128, 128, 128]);
    }

    #[test]
    fn yuv709_full_range_round_trip_within_two_levels() {
        for rgb in [[115u8, 82, 68], [98, 122, 157], [214, 126, 44], [8, 133, 161], [200, 200, 200]] {
            let [y, cb, cr] = rgb8_to_yuv709_full(rgb);
            let back = yuv709_full_to_rgb8(y, cb, cr);
            for c in 0..3 {
                assert!((i16::from(back[c]) - i16::from(rgb[c])).abs() <= 2, "{rgb:?} -> {back:?}");
            }
        }
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --locked color_metrics 2>&1 | tail -5`
Expected: compile error `cannot find function srgb8_to_lab` (and the other three).

- [ ] **Step 4: Implement the metrics**

Insert above the `#[cfg(test)] mod tests` block in `src/color_metrics.rs`:

```rust
/// sRGB 8-bit → CIELAB, D65 white.
pub fn srgb8_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    fn lin(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn f(t: f64) -> f64 {
        const D: f64 = 6.0 / 29.0;
        if t > D * D * D {
            t.cbrt()
        } else {
            t / (3.0 * D * D) + 4.0 / 29.0
        }
    }
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
    let (fx, fy, fz) = (f(x / 0.950_47), f(y), f(z / 1.088_83));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference (kL = kC = kH = 1).
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (lab1[0], lab1[1], lab1[2]);
    let (l2, a2, b2) = (lab2[0], lab2[1], lab2[2]);
    let pow25_7 = 25f64.powi(7);
    let c_bar = ((a1 * a1 + b1 * b1).sqrt() + (a2 * a2 + b2 * b2).sqrt()) / 2.0;
    let g = 0.5 * (1.0 - (c_bar.powi(7) / (c_bar.powi(7) + pow25_7)).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = ((a1p * a1p + b1 * b1).sqrt(), (a2p * a2p + b2 * b2).sqrt());
    let hue = |b: f64, ap: f64| {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let zero_chroma = c1p * c2p == 0.0;
    let dhp = if zero_chroma {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lp_bar = (l1 + l2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;
    let hp_bar = if zero_chroma {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cp_bar.powi(7) / (cp_bar.powi(7) + pow25_7)).sqrt();
    let l50 = (lp_bar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    let (tl, tc, th) = (dlp / sl, dcp / sc, dhp_big / sh);
    (tl * tl + tc * tc + th * th + rt * tc * th).sqrt()
}

fn clamp_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Full-range BT.709 Y'CbCr → R'G'B' (8-bit).
pub fn yuv709_full_to_rgb8(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let (y, cb, cr) = (f64::from(y), f64::from(cb) - 128.0, f64::from(cr) - 128.0);
    [
        clamp_u8(y + 1.5748 * cr),
        clamp_u8(y - 0.187_324 * cb - 0.468_124 * cr),
        clamp_u8(y + 1.8556 * cb),
    ]
}

/// Full-range BT.709 R'G'B' (8-bit) → Y'CbCr. Used only by tests to check the
/// inverse above.
pub fn rgb8_to_yuv709_full(rgb: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2]));
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    [clamp_u8(y), clamp_u8((b - y) / 1.8556 + 128.0), clamp_u8((r - y) / 1.5748 + 128.0)]
}
```

- [ ] **Step 5: Run the metric tests**

Run: `cargo test --locked color_metrics 2>&1 | grep -E 'test result|FAILED|panicked'`
Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 6: Write the failing pattern tests**

Create `src/color_pattern.rs` containing only the tests:

```rust
//! Deterministic BGRA test pattern for the color harness: the 24 ColorChecker
//! patches (sRGB) on a neutral background, plus a chroma-edge torture strip of
//! alternating 1-px red/blue columns (what 4:2:0 subsampling smears). Test-only.

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(p: &Pattern, x: usize, y: usize) -> [u8; 3] {
        let i = (y * p.width + x) * 4;
        [p.bgra[i + 2], p.bgra[i + 1], p.bgra[i]]
    }

    #[test]
    fn patches_have_their_colorchecker_colors() {
        for (w, h) in [(1920, 1080), (1713, 1288)] {
            let p = generate(w, h);
            assert_eq!(p.bgra.len(), w * h * 4);
            assert_eq!(p.patches.len(), 24);
            for (i, patch) in p.patches.iter().enumerate() {
                assert_eq!(patch.srgb, COLORCHECKER_SRGB[i]);
                let r = &patch.rect;
                assert!(r.w >= 16 && r.h >= 16, "patch {i} too small: {r:?}");
                assert!(r.x + r.w <= w && r.y + r.h <= h);
                assert_eq!(pixel(&p, r.x, r.y), patch.srgb);
                assert_eq!(pixel(&p, r.x + r.w - 1, r.y + r.h - 1), patch.srgb);
            }
        }
    }

    #[test]
    fn edge_strip_alternates_red_and_blue_columns() {
        let p = generate(1713, 1288);
        let e = &p.edge;
        assert!(e.w >= 64 && e.h >= 16);
        assert_eq!(pixel(&p, e.x, e.y), [255, 0, 0]);
        assert_eq!(pixel(&p, e.x + 1, e.y), [0, 0, 255]);
        assert_eq!(pixel(&p, e.x + 2, e.y + e.h - 1), [255, 0, 0]);
    }

    #[test]
    #[should_panic(expected = "pattern too small")]
    fn rejects_tiny_sizes() {
        let _ = generate(100, 100);
    }
}
```

- [ ] **Step 7: Run tests to verify they fail**

Run: `cargo test --locked color_pattern 2>&1 | tail -3`
Expected: compile error `cannot find function generate`.

- [ ] **Step 8: Implement the pattern**

Insert above the tests in `src/color_pattern.rs`:

```rust
/// X-Rite ColorChecker Classic, commonly published 8-bit sRGB values,
/// row-major (dark skin … black).
pub const COLORCHECKER_SRGB: [[u8; 3]; 24] = [
    [115, 82, 68], [194, 150, 130], [98, 122, 157], [87, 108, 67], [133, 128, 177], [103, 189, 170],
    [214, 126, 44], [80, 91, 166], [193, 90, 99], [94, 60, 108], [157, 188, 64], [224, 163, 46],
    [56, 61, 150], [70, 148, 73], [175, 54, 60], [231, 199, 31], [187, 86, 149], [8, 133, 161],
    [243, 243, 242], [200, 200, 200], [160, 160, 160], [122, 122, 121], [85, 85, 85], [52, 52, 52],
];

const BACKGROUND: [u8; 3] = [128, 128, 128];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub rect: Rect,
    pub srgb: [u8; 3],
}

pub struct Pattern {
    pub width: usize,
    pub height: usize,
    /// Tightly packed BGRA, stride = width * 4.
    pub bgra: Vec<u8>,
    pub patches: Vec<Patch>,
    /// Region of alternating 1-px red/blue columns.
    pub edge: Rect,
}

fn fill(bgra: &mut [u8], width: usize, r: Rect, rgb: [u8; 3]) {
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let i = (y * width + x) * 4;
            bgra[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
}

/// Top two thirds: 6×4 ColorChecker grid. Bottom third: chroma-edge strip.
pub fn generate(width: usize, height: usize) -> Pattern {
    assert!(width >= 6 * 48 && height >= 3 * 64, "pattern too small: {width}x{height}");
    let mut bgra = vec![0u8; width * height * 4];
    fill(&mut bgra, width, Rect { x: 0, y: 0, w: width, h: height }, BACKGROUND);

    let grid_h = height * 2 / 3;
    let (cell_w, cell_h) = (width / 6, grid_h / 4);
    let margin = (cell_w.min(cell_h) / 8).max(4);
    let mut patches = Vec::with_capacity(24);
    for (i, &srgb) in COLORCHECKER_SRGB.iter().enumerate() {
        let (col, row) = (i % 6, i / 6);
        let rect = Rect {
            x: col * cell_w + margin,
            y: row * cell_h + margin,
            w: cell_w - 2 * margin,
            h: cell_h - 2 * margin,
        };
        fill(&mut bgra, width, rect, srgb);
        patches.push(Patch { rect, srgb });
    }

    let edge = Rect { x: margin, y: grid_h + margin, w: width - 2 * margin, h: height - grid_h - 2 * margin };
    for y in edge.y..edge.y + edge.h {
        for x in edge.x..edge.x + edge.w {
            let rgb = if (x - edge.x) % 2 == 0 { [255, 0, 0] } else { [0, 0, 255] };
            let i = (y * width + x) * 4;
            bgra[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
    Pattern { width, height, bgra, patches, edge }
}
```

- [ ] **Step 9: Run the pattern tests**

Run: `cargo test --locked color_pattern 2>&1 | grep -E 'test result|FAILED|panicked'`
Expected: `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 10: Lint, format, commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/color_metrics.rs src/color_pattern.rs src/main.rs
git commit -m "test(color): CIEDE2000/BT.709 metrics and ColorChecker test pattern

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
Expected: clippy prints no warnings/errors.

---

### Task 5: Color round-trip harness (VideoToolbox → ffmpeg → ΔE)

**Files:**
- Create: `src/color_roundtrip_test.rs`
- Modify: `src/main.rs` (module declaration), `src/h264.rs:3186` (`fn avcc_to_annex_b` → `pub(crate) fn`), `FORK.md` (divergence row F2), `docs/research/phase0-baseline.md` (color section)

**Interfaces:**
- Consumes: `color_metrics::*`, `color_pattern::{generate, Pattern, Rect}` (Task 4); `crate::videotoolbox::Encoder::{new, encode_bgra, flush}` and `EncodedFrame { data, is_keyframe, parameter_sets, .. }` (upstream); `crate::h264::avcc_to_annex_b(&[u8], &[Vec<u8>], bool) -> Vec<u8>` (upstream, visibility widened here).
- Produces: `color_roundtrip_test::parse_probe(&str) -> Result<(usize, usize), String>`, `RoundTripReport { width, height, patch_mean_de, patch_max_de, edge_mean_de }`, and the ignored test `color_roundtrip_avc420_baseline`.

- [ ] **Step 1: Declare the module and widen visibility**

In `src/main.rs`, after the Task 4 declarations add:
```rust
#[cfg(all(test, target_os = "macos"))]
mod color_roundtrip_test;
```
In `src/h264.rs` change `fn avcc_to_annex_b(` to `pub(crate) fn avcc_to_annex_b(`.

- [ ] **Step 2: Write the failing parser test (Review Focus #1)**

Create `src/color_roundtrip_test.rs`:

```rust
//! End-to-end color fidelity of the H.264 path, without a client: pattern →
//! VideoToolbox (the production encoder) → Annex-B file → ffmpeg decode to raw
//! planes (no colour conversion) → full-range BT.709 → CIEDE2000 vs source.

use crate::color_metrics::{delta_e_2000, srgb8_to_lab, yuv709_full_to_rgb8};
use crate::color_pattern::{generate, Rect};

/// Parse `ffprobe -show_entries stream=width,height,pix_fmt -of default=noprint_wrappers=1`
/// output. Only 8-bit 4:2:0 planar output is measurable; anything else is refused.
pub fn parse_probe(out: &str) -> Result<(usize, usize), String> {
    let (mut w, mut h, mut fmt) = (None, None, None);
    for line in out.lines() {
        match line.split_once('=') {
            Some(("width", v)) => w = v.trim().parse::<usize>().ok(),
            Some(("height", v)) => h = v.trim().parse::<usize>().ok(),
            Some(("pix_fmt", v)) => fmt = Some(v.trim().to_string()),
            _ => {}
        }
    }
    match (w, h, fmt.as_deref()) {
        (Some(w), Some(h), Some("yuv420p" | "yuvj420p")) => Ok((w, h)),
        (_, _, Some(other)) if other != "yuv420p" && other != "yuvj420p" => {
            Err(format!("unsupported pix_fmt {other}; refusing to measure"))
        }
        _ => Err(format!("incomplete ffprobe output: {out:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_probe_accepts_8bit_420() {
        assert_eq!(parse_probe("width=1713\nheight=1288\npix_fmt=yuvj420p\n"), Ok((1713, 1288)));
        assert_eq!(parse_probe("pix_fmt=yuv420p\nwidth=1920\nheight=1080"), Ok((1920, 1080)));
    }

    #[test]
    fn parse_probe_refuses_other_formats() {
        let e = parse_probe("width=1920\nheight=1080\npix_fmt=yuv420p10le\n").unwrap_err();
        assert!(e.contains("yuv420p10le"), "{e}");
        assert!(parse_probe("width=1920\n").is_err());
    }
}
```

- [ ] **Step 3: Run the parser tests**

Run: `cargo test --locked color_roundtrip_test::tests 2>&1 | grep -E 'test result|FAILED'`
Expected: `test result: ok. 2 passed; 0 failed` (the parser is written with its tests; if either fails, fix `parse_probe` before continuing).

- [ ] **Step 4: Write the round-trip test**

Append to `src/color_roundtrip_test.rs` (outside `mod tests`):

```rust
#[derive(Debug)]
pub struct RoundTripReport {
    pub width: usize,
    pub height: usize,
    pub patch_mean_de: f64,
    pub patch_max_de: f64,
    pub edge_mean_de: f64,
}

impl RoundTripReport {
    fn to_json(&self) -> String {
        format!(
            "{{\"width\":{},\"height\":{},\"patch_mean_de\":{:.3},\"patch_max_de\":{:.3},\"edge_mean_de\":{:.3}}}",
            self.width, self.height, self.patch_mean_de, self.patch_max_de, self.edge_mean_de
        )
    }
}

fn run(cmd: &mut std::process::Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("spawn {cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(out.stdout)
}

fn mean_de(src: &[u8], dec: &[[u8; 3]], width: usize, r: &Rect) -> f64 {
    let mut sum = 0.0;
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let i = y * width + x;
            let s = [src[i * 4 + 2], src[i * 4 + 1], src[i * 4]];
            sum += delta_e_2000(srgb8_to_lab(s), srgb8_to_lab(dec[i]));
        }
    }
    sum / (r.w * r.h) as f64
}

pub fn run_roundtrip(width: usize, height: usize) -> Result<RoundTripReport, String> {
    let pattern = generate(width, height);
    let mut enc = crate::videotoolbox::Encoder::new(width as u16, height as u16, 60, 50_000_000, 2.0)
        .map_err(|e| e.to_string())?;
    // A static screen is re-sent as P-frames; measure the converged last frame.
    for i in 0..30 {
        enc.encode_bgra(&pattern.bgra, width * 4, i == 0).map_err(|e| e.to_string())?;
    }
    let frames = enc.flush().map_err(|e| e.to_string())?;
    if frames.is_empty() {
        return Err("encoder produced no frames".into());
    }
    let mut annexb = Vec::new();
    for f in &frames {
        annexb.extend(crate::h264::avcc_to_annex_b(&f.data, &f.parameter_sets, f.is_keyframe));
    }
    let path = std::env::temp_dir().join(format!("macrdp-color-{width}x{height}.h264"));
    std::fs::write(&path, &annexb).map_err(|e| e.to_string())?;

    let probe = run(std::process::Command::new("ffprobe").args([
        "-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=width,height,pix_fmt",
        "-of", "default=noprint_wrappers=1",
    ]).arg(&path))?;
    let (dw, dh) = parse_probe(&String::from_utf8_lossy(&probe))?;
    if dw < width || dh < height {
        return Err(format!("decoded {dw}x{dh} smaller than source {width}x{height}"));
    }
    // No -pix_fmt: raw decoded planes, so ffmpeg performs no colour conversion.
    let raw = run(std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-f", "rawvideo", "-"]))?;
    let (cw, ch) = (dw.div_ceil(2), dh.div_ceil(2));
    let frame_len = dw * dh + 2 * cw * ch;
    if raw.len() < frame_len || raw.len() % frame_len != 0 {
        return Err(format!("raw output {} bytes is not a whole number of {frame_len}-byte frames", raw.len()));
    }
    let last = &raw[raw.len() - frame_len..];
    let (yp, rest) = last.split_at(dw * dh);
    let (up, vp) = rest.split_at(cw * ch);

    // Crop to the source size (Review Focus #2).
    let mut dec = vec![[0u8; 3]; width * height];
    for y in 0..height {
        for x in 0..width {
            let ci = (y / 2) * cw + x / 2;
            dec[y * width + x] = yuv709_full_to_rgb8(yp[y * dw + x], up[ci], vp[ci]);
        }
    }

    let inset = |r: &Rect| Rect { x: r.x + 8, y: r.y + 8, w: r.w - 16, h: r.h - 16 };
    let des: Vec<f64> = pattern.patches.iter().map(|p| mean_de(&pattern.bgra, &dec, width, &inset(&p.rect))).collect();
    Ok(RoundTripReport {
        width,
        height,
        patch_mean_de: des.iter().sum::<f64>() / des.len() as f64,
        patch_max_de: des.iter().cloned().fold(0.0, f64::max),
        edge_mean_de: mean_de(&pattern.bgra, &dec, width, &pattern.edge),
    })
}

#[test]
#[ignore = "needs VideoToolbox + ffmpeg; run: cargo test --release color_roundtrip -- --ignored --nocapture"]
fn color_roundtrip_avc420_baseline() {
    for (w, h) in [(1920, 1080), (1713, 1288)] {
        let report = run_roundtrip(w, h).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        println!("COLOR_REPORT {}", report.to_json());
        // Sanity gate only: flat patches must survive AVC420 nearly intact.
        // Phase 2 tightens this to ΔE < 1 (AVC444) and bit-exact (refinement).
        assert!(report.patch_mean_de < 2.0, "{report:?}");
    }
}
```

- [ ] **Step 5: Run the harness**

Run: `cargo test --locked --release color_roundtrip -- --ignored --nocapture 2>&1 | grep -E 'COLOR_REPORT|test result|panicked'`
Expected: two `COLOR_REPORT {...}` lines and `test result: ok. 1 passed` (plus the 2 parser tests). `edge_mean_de` is expected to be large (4:2:0 smears 1-px red/blue columns) — that is the baseline Phase 2 must beat.

- [ ] **Step 6: Record results and the divergence**

Append to `docs/research/phase0-baseline.md`:

```markdown
## Color (AVC420, VideoToolbox, 50 Mbps, 30 static frames, ffmpeg decode, full-range BT.709)
| Size | Patch mean ΔE00 | Patch max ΔE00 | Chroma-edge mean ΔE00 |
|------|-----------------|----------------|-----------------------|
| 1920×1080 | <patch_mean_de> | <patch_max_de> | <edge_mean_de> |
| 1713×1288 | <patch_mean_de> | <patch_max_de> | <edge_mean_de> |
Command: `cargo test --locked --release color_roundtrip -- --ignored --nocapture`
```
Replace each `<…>` with the values from the two `COLOR_REPORT` lines.

Append to the divergence log in `FORK.md`:
```markdown
| F2 | `src/h264.rs` | `avcc_to_annex_b` → `pub(crate)` | Reused by the color round-trip harness | Yes (trivial) |
```

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
git add src/color_roundtrip_test.rs src/main.rs src/h264.rs FORK.md docs/research/phase0-baseline.md
git commit -m "test(color): VideoToolbox→ffmpeg round-trip ΔE harness + AVC420 baseline

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Process sampler (CPU from cumulative CPU time, RSS)

**Files:**
- Create: `scripts/perf/perfsample.py`, `scripts/perf/test_perfsample.py`

**Interfaces:**
- Produces: CLI `python3 scripts/perf/perfsample.py sample <pid> <seconds> <out.csv>` (CSV header `t,cpu_pct,rss_mb`; exits 1 if the process disappears) and `python3 scripts/perf/perfsample.py summarize <csv>` (prints `cpu_mean_pct=… cpu_p95_pct=… rss_max_mb=…`). Python functions `parse_cputime(str) -> float`, `summarize(rows) -> dict`.

- [ ] **Step 1: Write the failing tests (includes Review Focus #3)**

Create `scripts/perf/test_perfsample.py`:

```python
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import perfsample  # noqa: E402

HERE = os.path.dirname(__file__)


class ParseCputime(unittest.TestCase):
    def test_minutes_seconds(self):
        self.assertAlmostEqual(perfsample.parse_cputime("0:01.23"), 1.23)
        self.assertAlmostEqual(perfsample.parse_cputime("12:00.50"), 720.5)

    def test_hours(self):
        self.assertAlmostEqual(perfsample.parse_cputime("1:02:03.00"), 3723.0)


class Summarize(unittest.TestCase):
    def test_mean_p95_max(self):
        rows = [(i, float(i), 100.0 + i) for i in range(1, 21)]  # cpu 1..20
        s = perfsample.summarize(rows)
        self.assertAlmostEqual(s["cpu_mean_pct"], 10.5)
        self.assertAlmostEqual(s["cpu_p95_pct"], 19.0)
        self.assertAlmostEqual(s["rss_max_mb"], 120.0)

    def test_empty_is_error(self):
        with self.assertRaises(ValueError):
            perfsample.summarize([])


class SampleCli(unittest.TestCase):
    def test_samples_a_live_process(self):
        p = subprocess.Popen(["sleep", "30"])
        try:
            with tempfile.TemporaryDirectory() as d:
                out = os.path.join(d, "s.csv")
                r = subprocess.run([sys.executable, os.path.join(HERE, "perfsample.py"), "sample", str(p.pid), "3", out])
                self.assertEqual(r.returncode, 0)
                lines = open(out).read().strip().splitlines()
                self.assertEqual(lines[0], "t,cpu_pct,rss_mb")
                self.assertEqual(len(lines), 4)
        finally:
            p.kill()

    def test_sample_exits_when_process_dies(self):
        p = subprocess.Popen(["sleep", "1"])
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "s.csv")
            r = subprocess.run(
                [sys.executable, os.path.join(HERE, "perfsample.py"), "sample", str(p.pid), "5", out],
                capture_output=True, text=True,
            )
            self.assertEqual(r.returncode, 1)
            self.assertIn("exited", r.stderr)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run to verify failure**

Run: `python3 -m unittest discover -s scripts/perf -v 2>&1 | tail -3`
Expected: `ModuleNotFoundError: No module named 'perfsample'`.

- [ ] **Step 3: Implement the sampler**

Create `scripts/perf/perfsample.py`:

```python
#!/usr/bin/env python3
"""Sample one process's CPU% (from cumulative CPU time deltas, not ps's decaying
average) and RSS once per second; summarize a CSV of samples."""
import csv
import subprocess
import sys
import time


def parse_cputime(s: str) -> float:
    """ps 'time' field: [[H:]M]:S.ss → seconds."""
    total = 0.0
    for part in s.strip().split(":"):
        total = total * 60 + float(part)
    return total


def _read(pid: int):
    r = subprocess.run(["ps", "-o", "time=,rss=", "-p", str(pid)], capture_output=True, text=True)
    fields = r.stdout.split()
    if r.returncode != 0 or len(fields) != 2:
        return None
    return parse_cputime(fields[0]), int(fields[1]) / 1024.0


def sample(pid: int, seconds: int, out_path: str) -> int:
    prev = _read(pid)
    if prev is None:
        print(f"process {pid} not found", file=sys.stderr)
        return 1
    t_prev = time.monotonic()
    with open(out_path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["t", "cpu_pct", "rss_mb"])
        for i in range(1, seconds + 1):
            time.sleep(max(0.0, t_prev + 1.0 - time.monotonic()))
            cur = _read(pid)
            now = time.monotonic()
            if cur is None:
                print(f"process {pid} exited after {i - 1}s; results invalid", file=sys.stderr)
                return 1
            cpu = (cur[0] - prev[0]) / (now - t_prev) * 100.0
            w.writerow([i, f"{cpu:.2f}", f"{cur[1]:.1f}"])
            prev, t_prev = cur, now
    return 0


def summarize(rows):
    if not rows:
        raise ValueError("no samples")
    cpus = sorted(r[1] for r in rows)
    p95 = cpus[max(0, int(round(0.95 * len(cpus))) - 1)]
    return {
        "cpu_mean_pct": sum(cpus) / len(cpus),
        "cpu_p95_pct": p95,
        "rss_max_mb": max(r[2] for r in rows),
    }


def _load(path: str):
    with open(path) as f:
        return [(int(r["t"]), float(r["cpu_pct"]), float(r["rss_mb"])) for r in csv.DictReader(f)]


def main(argv) -> int:
    if len(argv) == 5 and argv[1] == "sample":
        return sample(int(argv[2]), int(argv[3]), argv[4])
    if len(argv) == 3 and argv[1] == "summarize":
        s = summarize(_load(argv[2]))
        print(" ".join(f"{k}={v:.2f}" for k, v in s.items()))
        return 0
    print("usage: perfsample.py sample <pid> <seconds> <out.csv> | summarize <csv>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
```

- [ ] **Step 4: Run tests**

Run: `python3 -m unittest discover -s scripts/perf -v 2>&1 | tail -3`
Expected: `Ran 6 tests` … `OK`.

- [ ] **Step 5: Commit**

```bash
chmod +x scripts/perf/perfsample.py
git add scripts/perf/perfsample.py scripts/perf/test_perfsample.py
git commit -m "test(perf): per-process CPU/RSS sampler with dead-process detection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Workload generator (Swift/AppKit)

**Files:**
- Create: `tools/workload/main.swift`, `tools/workload/build.sh`
- Modify: `.gitignore` (add `perf-results/`)

**Interfaces:**
- Produces: `target/workload <idle|typing|scroll|motion> <seconds> [screen-name-substring]` — a borderless full-screen window on the first `NSScreen` whose `localizedName` contains the substring (default: main screen); exits after `<seconds>`; exit code 2 on bad arguments, 3 if no screen matches.

- [ ] **Step 1: Write the generator**

Create `tools/workload/main.swift`:

```swift
// Deterministic desktop workloads for the performance harness.
// Build: tools/workload/build.sh   Run: target/workload typing 60 macrdp
import AppKit

enum Mode: String { case idle, typing, scroll, motion }

final class WorkloadView: NSView {
    let mode: Mode
    var tick = 0
    var typed = ""
    let source = Array("fn main() { let total: u64 = (1..=100).sum(); println!(\"{total}\"); } // ")

    init(mode: Mode, frame: NSRect) {
        self.mode = mode
        super.init(frame: frame)
    }
    required init?(coder: NSCoder) { fatalError("unused") }
    override var isFlipped: Bool { true }

    func step() {
        tick += 1
        switch mode {
        case .idle:
            return
        case .typing:
            guard tick % 6 == 0 else { return } // 10 chars/s at 60 Hz
            typed.append(source[(tick / 6) % source.count])
            if typed.count > 4000 { typed = "" }
            needsDisplay = true
        case .scroll, .motion:
            needsDisplay = true
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(srgbRed: 0.12, green: 0.12, blue: 0.14, alpha: 1).setFill()
        bounds.fill()
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 14, weight: .regular),
            .foregroundColor: NSColor(srgbRed: 0.85, green: 0.85, blue: 0.85, alpha: 1),
        ]
        switch mode {
        case .idle:
            ("idle workload: static content" as NSString).draw(at: NSPoint(x: 40, y: 40), withAttributes: attrs)
        case .typing:
            (typed as NSString).draw(in: bounds.insetBy(dx: 40, dy: 40), withAttributes: attrs)
        case .scroll:
            let lineHeight: CGFloat = 20
            let offset = CGFloat(tick) // 60 px/s
            let first = Int(offset / lineHeight)
            for i in 0..<(Int(bounds.height / lineHeight) + 2) {
                let line = first + i
                let text = "\(line): let value_\(line) = compute(\(line * 7 % 101)); // scrolling source line"
                (text as NSString).draw(at: NSPoint(x: 40, y: CGFloat(line) * lineHeight - offset), withAttributes: attrs)
            }
        case .motion:
            let t = CGFloat(tick) / 60
            let gradient = NSGradient(colors: [
                NSColor(srgbRed: (sin(t) + 1) / 2, green: 0.3, blue: 0.6, alpha: 1),
                NSColor(srgbRed: 0.1, green: (cos(t * 1.3) + 1) / 2, blue: 0.4, alpha: 1),
            ])!
            gradient.draw(in: bounds, angle: CGFloat(tick % 360))
        }
    }
}

let args = CommandLine.arguments
guard args.count >= 3, let mode = Mode(rawValue: args[1]), let seconds = Double(args[2]) else {
    FileHandle.standardError.write("usage: workload idle|typing|scroll|motion <seconds> [screen-name-substring]\n".data(using: .utf8)!)
    exit(2)
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let screen: NSScreen
if args.count >= 4 {
    guard let match = NSScreen.screens.first(where: { $0.localizedName.contains(args[3]) }) else {
        let names = NSScreen.screens.map(\.localizedName).joined(separator: ", ")
        FileHandle.standardError.write("no screen matching '\(args[3])'; screens: \(names)\n".data(using: .utf8)!)
        exit(3)
    }
    screen = match
} else {
    screen = NSScreen.main!
}
let window = NSWindow(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false, screen: screen)
let view = WorkloadView(mode: mode, frame: NSRect(origin: .zero, size: screen.frame.size))
window.contentView = view
window.setFrame(screen.frame, display: true)
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) { _ in view.step() }
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
app.run()
```

Create `tools/workload/build.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$root/target"
swiftc -O -swift-version 5 "$root/tools/workload/main.swift" -o "$root/target/workload"
echo "built $root/target/workload"
```

Append `perf-results/` to `.gitignore`.

- [ ] **Step 2: Build and check argument handling**

```bash
chmod +x tools/workload/build.sh && tools/workload/build.sh
target/workload; echo "exit=$?"
target/workload typing 2 no-such-screen; echo "exit=$?"
```
Expected: `built …`; usage line and `exit=2`; `no screen matching 'no-such-screen'; screens: …` and `exit=3`.

- [ ] **Step 3: Visual check of each mode**

```bash
for m in idle typing scroll motion; do target/workload $m 3 & sleep 2; screencapture -x "/tmp/workload-$m.png"; wait; done; ls -la /tmp/workload-*.png
```
Expected: four PNGs; open them and confirm: static text; growing code text; scrolled source lines; a colour gradient.

- [ ] **Step 4: Commit**

```bash
git add tools/workload .gitignore
git commit -m "test(perf): Swift workload generator (idle/typing/scroll/motion)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Host performance runner + macrdp baseline

**Files:**
- Create: `scripts/perf/run-host-perf.sh`, `docs/perf/README.md`
- Modify: `docs/research/phase0-baseline.md` (host section)

**Interfaces:**
- Consumes: `perfsample.py` (Task 6), `target/workload` (Task 7), `target/release/macrdp`, `sdl-freerdp`.
- Produces: `scripts/perf/run-host-perf.sh <label> <workload> <seconds> [process-name]` → `perf-results/<timestamp>-<label>-<workload>/{samples.csv,summary.txt,server.log,client.log}`.

- [ ] **Step 1: Write the runner**

Create `scripts/perf/run-host-perf.sh`:

```bash
#!/usr/bin/env bash
# Measure an RDP server process on this Mac while a workload runs.
#   macrdp (default): builds nothing; launches target/release/macrdp on a
#     1920x1080 virtual display plus a loopback sdl-freerdp client. The workload
#     runs on the virtual display, so the client window (on the physical
#     screen) is never captured (no mirror feedback).
#   other process name (e.g. "Jump Desktop Connect"): connect from Windows first,
#     make sure the remote view shows the display the workload will use, then run.
# Usage: run-host-perf.sh <label> <idle|typing|scroll|motion> <seconds> [process-name]
set -euo pipefail
label=$1 workload=$2 seconds=$3 target=${4:-macrdp}
root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/perf-results/$(date +%Y%m%d-%H%M%S)-$label-$workload"
mkdir -p "$out"
server_pid="" client_pid=""
cleanup() {
  [[ -n "$client_pid" ]] && kill "$client_pid" 2>/dev/null || true
  [[ -n "$server_pid" ]] && kill "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT

screen_arg=()
if [[ "$target" == macrdp ]]; then
  [[ -x "$root/target/release/macrdp" ]] || { echo "build first: cargo build --release --locked" >&2; exit 2; }
  "$root/target/release/macrdp" --bind 127.0.0.1:3390 --skip-auth --password perf \
    --virtual-display --width 1920 --height 1080 --enable-h264 --adaptive-bitrate \
    >"$out/server.log" 2>&1 &
  server_pid=$!
  sleep 3
  kill -0 "$server_pid" 2>/dev/null || { echo "macrdp exited; see $out/server.log" >&2; exit 1; }
  frdp=$(command -v sdl-freerdp || command -v sdl3-freerdp)
  "$frdp" /v:127.0.0.1:3390 /u:"$USER" /p:perf /cert:ignore /gfx:avc444 /size:1920x1080 \
    >"$out/client.log" 2>&1 &
  client_pid=$!
  sleep 5
  pid=$server_pid
  screen_arg=(macrdp)
else
  pid=$(pgrep -x "$target" | head -1) || { echo "no running process named '$target'" >&2; exit 1; }
fi

"$root/target/workload" "$workload" "$((seconds + 3))" "${screen_arg[@]}" &
sleep 2
python3 "$root/scripts/perf/perfsample.py" sample "$pid" "$seconds" "$out/samples.csv"
python3 "$root/scripts/perf/perfsample.py" summarize "$out/samples.csv" | tee "$out/summary.txt"
echo "results: $out"
```

- [ ] **Step 2: Grant permissions once (manual)**

Build: `cargo build --release --locked`. Then run `target/release/macrdp --skip-auth --password x` once from Terminal; approve **Screen Recording** and **Accessibility** for the terminal app when macOS prompts (System Settings → Privacy & Security). Stop it with Ctrl-C.

- [ ] **Step 3: Idle sanity run (Review Focus #5)**

Run: `chmod +x scripts/perf/run-host-perf.sh && scripts/perf/run-host-perf.sh macrdp idle 30`
Expected: a `cpu_mean_pct=… cpu_p95_pct=… rss_max_mb=…` line. **Check:** `cpu_mean_pct` should be low single digits. If it is ≥ 20%, the client window is being captured (mirror feedback) — confirm in `server.log` that the virtual display is the captured display and that the workload window appeared on it, and fix the runner before recording anything.

- [ ] **Step 4: Record macrdp baselines**

Run each: `for w in idle typing scroll motion; do scripts/perf/run-host-perf.sh macrdp $w 60; done`

Append to `docs/research/phase0-baseline.md`:

```markdown
## Host performance — macrdp (upstream behavior; 1920×1080 virtual display, H.264, adaptive bitrate, loopback sdl-freerdp /gfx:avc444)
| Workload | CPU mean % | CPU p95 % | RSS max MB |
|----------|-----------|-----------|------------|
| idle | <cpu_mean_pct> | <cpu_p95_pct> | <rss_max_mb> |
| typing | … | … | … |
| scroll | … | … | … |
| motion | … | … | … |
CPU % is of one core. Budgets (spec §9): idle < 1%, active 4K@60 < 15%.
```
Fill every cell from the matching `summary.txt`; do not leave `…` or `<…>`.

- [ ] **Step 5: Write `docs/perf/README.md`**

```markdown
# Performance harness

## Host (this Mac)
1. `cargo build --release --locked && tools/workload/build.sh`
2. `scripts/perf/run-host-perf.sh <label> <idle|typing|scroll|motion> <seconds> [process-name]`
3. Results land in `perf-results/` (git-ignored); copy the summary into `docs/research/phase0-baseline.md`.

The default target launches macrdp on a virtual display with a loopback FreeRDP
client. To measure another server (e.g. Jump Desktop Connect), connect to it from
Windows first, then pass its process name.

## Client (Windows PC)
`scripts/perf/client-perf.ps1 -ProcessName mstsc -Seconds 60` while the same
workload runs on the Mac. Use `-ProcessName JumpDesktop` for Jump Desktop.

## Workloads
`target/workload <mode> <seconds> [screen-name-substring]` — idle, typing (10 chars/s),
scroll (60 px/s), motion (full-screen animated gradient at 60 Hz).
```

- [ ] **Step 6: Commit**

```bash
chmod +x scripts/perf/run-host-perf.sh
git add scripts/perf/run-host-perf.sh docs/perf/README.md docs/research/phase0-baseline.md
git commit -m "test(perf): host perf runner and macrdp baseline numbers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Windows client sampler + Jump Desktop baseline (user runs on Windows)

**Files:**
- Create: `scripts/perf/client-perf.ps1`
- Modify: `docs/research/phase0-baseline.md` (client + Jump section)

**Interfaces:**
- Produces: `client-perf.ps1 -ProcessName <name> -Seconds <n>` → prints `cpu_mean_pct=… cpu_p95_pct=… gpu_mean_pct=… ws_max_mb=…`, sums all instances of the process, exits 1 if none are running.

- [ ] **Step 1: Write the script**

Create `scripts/perf/client-perf.ps1`:

```powershell
# Sample an RDP client's CPU (% of one core, all instances summed), GPU engine
# utilization and working set once per second.
# Usage: .\client-perf.ps1 -ProcessName mstsc -Seconds 60
param(
    [Parameter(Mandatory = $true)][string]$ProcessName,
    [int]$Seconds = 60
)
$ErrorActionPreference = 'Stop'
$procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
if (-not $procs) { Write-Error "no running process named '$ProcessName'"; exit 1 }
$pids = @($procs.Id)
$cpu = @(); $gpu = @(); $ws = @()
$prevTicks = ($procs | ForEach-Object { $_.TotalProcessorTime.Ticks } | Measure-Object -Sum).Sum
$clock = [Diagnostics.Stopwatch]::StartNew(); $last = 0
for ($i = 1; $i -le $Seconds; $i++) {
    Start-Sleep -Milliseconds ([Math]::Max(0, $i * 1000 - $clock.ElapsedMilliseconds))
    $now = Get-Process -Id $pids -ErrorAction SilentlyContinue
    if (-not $now) { Write-Error "process exited after $($i - 1)s; results invalid"; exit 1 }
    $ticks = ($now | ForEach-Object { $_.TotalProcessorTime.Ticks } | Measure-Object -Sum).Sum
    $elapsed = $clock.ElapsedMilliseconds
    $cpu += (($ticks - $prevTicks) / 10000.0) / ($elapsed - $last) * 100.0
    $prevTicks = $ticks; $last = $elapsed
    $ws += (($now | Measure-Object -Property WorkingSet64 -Sum).Sum / 1MB)
    $g = 0.0
    foreach ($p in $pids) {
        $samples = (Get-Counter "\GPU Engine(pid_$($p)_*)\Utilization Percentage" -ErrorAction SilentlyContinue).CounterSamples
        if ($samples) { $g += ($samples | Measure-Object -Property CookedValue -Sum).Sum }
    }
    $gpu += $g
}
$sorted = $cpu | Sort-Object
$p95 = $sorted[[Math]::Max(0, [Math]::Round(0.95 * $sorted.Count) - 1)]
"cpu_mean_pct={0:N2} cpu_p95_pct={1:N2} gpu_mean_pct={2:N2} ws_max_mb={3:N1}" -f `
    ($cpu | Measure-Object -Average).Average, $p95, ($gpu | Measure-Object -Average).Average, ($ws | Measure-Object -Maximum).Maximum
```

- [ ] **Step 2: Syntax check on the Mac (if PowerShell is available)**

Run: `command -v pwsh && pwsh -NoProfile -Command "[System.Management.Automation.Language.Parser]::ParseFile('scripts/perf/client-perf.ps1',[ref]\$null,[ref]\$e) > \$null; if (\$e) { \$e; exit 1 } else { 'parse ok' }" || echo "pwsh not installed; parse check happens on Windows in Step 3"`
Expected: `parse ok`, or the not-installed message.

- [ ] **Step 3: Manual baseline session (user, on the Windows PC + this Mac)**

For each of **(a) mstsc → macrdp** and **(b) Jump Desktop client → Jump Desktop Connect**:
1. Connect from Windows in a window of 1920×1080 on the ultrawide.
2. On the Mac, run the workload on the displayed screen: `target/workload <mode> 70` (for macrdp without loopback: start `target/release/macrdp --virtual-display --width 1920 --height 1080 --enable-h264 --adaptive-bitrate` first and pass `macrdp` as the screen name).
3. On Windows, within 5 s: `powershell -ExecutionPolicy Bypass -File client-perf.ps1 -ProcessName <mstsc|JumpDesktop> -Seconds 60`.
4. For (b) also run on the Mac: `scripts/perf/run-host-perf.sh jump <mode> 60 "Jump Desktop Connect"`.
Do this for `idle`, `scroll` and `motion`.

- [ ] **Step 4: Record**

Append to `docs/research/phase0-baseline.md`:

```markdown
## Client + Jump Desktop comparison (Windows client, 1920×1080 window)
| Workload | mstsc CPU % | mstsc GPU % | Jump client CPU % | Jump client GPU % | Jump host CPU % | Jump host RSS MB |
|----------|-------------|-------------|-------------------|-------------------|-----------------|------------------|
| idle | | | | | | |
| scroll | | | | | | |
| motion | | | | | | |
Windows PC: <model, CPU, GPU, Windows version>.
```
Fill every cell and the PC line with measured values.

- [ ] **Step 5: Commit**

```bash
git add scripts/perf/client-perf.ps1 docs/research/phase0-baseline.md
git commit -m "test(perf): Windows client sampler and Jump Desktop comparison baseline

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 10: Multi-session spike (THROWAWAY — output is a verdict)

**Files:**
- Create: `spikes/multisession/README.md`, `spikes/multisession/probe/main.swift`, `spikes/multisession/probe/Bridging.h`, `spikes/multisession/probe/build.sh`
- Create: `docs/research/2026-09-29-multisession-spike.md` (verdict; date is the day it's written)

**Interfaces:**
- Produces: `spikes/multisession/probe/probe` — prints one JSON line `{"user":…,"console":bool,"virtual_display":ok|error,"capture":ok|error,"input":ok|error}` describing what works from the session it runs in.
- Produces for Phase 3: the verdict document naming the mechanism for simultaneous sessions and the per-user TCC finding.

- [ ] **Step 1: Mark the spike throwaway**

Create `spikes/multisession/README.md`:

```markdown
# Multi-session spike (throwaway)
Question (spec §5.3): how do Apple Screen Sharing and Jump Desktop run simultaneous
remote sessions for different users, and can our session agent create virtual
displays, capture, and inject input inside a background (non-console) session?
Nothing here ships. The deliverable is docs/research/*-multisession-spike.md.
```

- [ ] **Step 2: Study how the shipping products do it**

```bash
J="/Applications/Jump Desktop Connect.app"
ls "$J/Contents" "$J/Contents/Library/LaunchServices" "$J/Contents/Library/LaunchDaemons" 2>/dev/null
codesign -d --entitlements - "$J" 2>/dev/null | head -60
find "$J" -type f -perm -u+x | while read -r b; do echo "== $b"; strings -a "$b" | grep -E -i 'CGSSession|SACSession|SACLogin|loginwindow|CGVirtualDisplay|RemoteDesktopSession|headless|virtual session|asuser|bootstrap' | sort -u | head -40; done
SS=/System/Library/CoreServices/RemoteManagement/screensharingd.bundle/Contents/MacOS/screensharingd
strings -a "$SS" | grep -E -i 'SACSession|virtual|session|loginwindow|CGS' | sort -u | head -80
ls /System/Library/PrivateFrameworks | grep -i -E 'login|session|screensharing|remote'
launchctl print system | grep -i -E 'jump|screensharing' | head
```
Record every symbol, framework and launchd job that looks session-related in the verdict doc draft (Step 6), with the command that found it.

- [ ] **Step 3: Write the probe**

Create `spikes/multisession/probe/Bridging.h` (private API declarations, as used by macrdp's `src/virtual_display/private_api.rs`):

```objc
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>

@interface CGVirtualDisplayDescriptor : NSObject
@property (retain, nonatomic) dispatch_queue_t queue;
@property (retain, nonatomic) NSString *name;
@property (nonatomic) unsigned int maxPixelsWide;
@property (nonatomic) unsigned int maxPixelsHigh;
@property (nonatomic) CGSize sizeInMillimeters;
@property (nonatomic) unsigned int productID;
@property (nonatomic) unsigned int vendorID;
@property (nonatomic) unsigned int serialNum;
@end

@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)width height:(unsigned int)height refreshRate:(double)refreshRate;
@end

@interface CGVirtualDisplaySettings : NSObject
@property (retain, nonatomic) NSArray<CGVirtualDisplayMode *> *modes;
@property (nonatomic) unsigned int hiDPI;
@end

@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@property (readonly, nonatomic) unsigned int displayID;
@end
```

Create `spikes/multisession/probe/main.swift`:

```swift
import AppKit
import ScreenCaptureKit

func esc(_ s: String) -> String { s.replacingOccurrences(of: "\"", with: "'") }

var result: [String: String] = ["user": NSUserName()]
let session = CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]
result["console"] = String(describing: session["kCGSSessionOnConsoleKey"] ?? "unknown")

// 1. Virtual display
let desc = CGVirtualDisplayDescriptor()
desc.queue = DispatchQueue.main
desc.name = "spike"
desc.maxPixelsWide = 1920; desc.maxPixelsHigh = 1080
desc.sizeInMillimeters = CGSize(width: 600, height: 340)
desc.productID = 0x5350; desc.vendorID = 0x5350; desc.serialNum = 1
let vd = CGVirtualDisplay(descriptor: desc)
let settings = CGVirtualDisplaySettings()
settings.modes = [CGVirtualDisplayMode(width: 1920, height: 1080, refreshRate: 60)]
settings.hiDPI = 0
result["virtual_display"] = (vd.applySettings(settings) && vd.displayID != 0) ? "ok id=\(vd.displayID)" : "error applySettings"

// 2. Input injection (a harmless mouse move onto the virtual display)
if let ev = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: CGPoint(x: 10, y: 10), mouseButton: .left) {
    ev.post(tap: .cghidEventTap)
    result["input"] = AXIsProcessTrusted() ? "ok (posted, AX trusted)" : "error (posted but process not AX-trusted)"
} else {
    result["input"] = "error CGEvent nil"
}

// 3. One ScreenCaptureKit frame of the virtual display
let done = DispatchSemaphore(value: 0)
Task {
    do {
        try await Task.sleep(nanoseconds: 1_000_000_000)
        let content = try await SCShareableContent.current
        guard let d = content.displays.first(where: { $0.displayID == vd.displayID }) ?? content.displays.first else {
            result["capture"] = "error no displays"; done.signal(); return
        }
        let cfg = SCStreamConfiguration(); cfg.width = 640; cfg.height = 360
        let img = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(display: d, excludingWindows: []), configuration: cfg)
        result["capture"] = "ok \(img.width)x\(img.height) display=\(d.displayID)"
    } catch {
        result["capture"] = "error \(esc(error.localizedDescription))"
    }
    done.signal()
}
while done.wait(timeout: .now() + 0.05) == .timedOut { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
print("{" + result.sorted { $0.key < $1.key }.map { "\"\($0.key)\":\"\(esc($0.value))\"" }.joined(separator: ",") + "}")
```

Create `spikes/multisession/probe/build.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
swiftc -O -swift-version 5 -import-objc-header Bridging.h main.swift -o probe \
  -framework AppKit -framework ScreenCaptureKit -framework CoreGraphics
echo "built $(pwd)/probe"
```

- [ ] **Step 4: Run the probe in the console session**

```bash
chmod +x spikes/multisession/probe/build.sh && spikes/multisession/probe/build.sh
spikes/multisession/probe/probe
```
Expected: JSON with `"console":"1"` (or `true`), `virtual_display` ok, `capture` ok (after granting Screen Recording to the terminal), `input` ok (after granting Accessibility).

- [ ] **Step 5: Run the probe in a background session (user assists)**

1. **User:** create a standard test account `rdpspike` in System Settings → Users & Groups, enable Fast User Switching, log into `rdpspike` once via the menu-bar user switcher, then switch back to your own account (leaving `rdpspike` logged in in the background).
2. Copy the probe where the test user can run it: `sudo cp spikes/multisession/probe/probe /Users/Shared/probe && sudo chmod 755 /Users/Shared/probe`
3. Run it inside rdpspike's background GUI session:
```bash
uid=$(id -u rdpspike)
sudo launchctl asuser "$uid" sudo -u rdpspike /Users/Shared/probe
```
4. Record the JSON. Expected unknowns: capture and input likely report TCC errors for the new user (answers the per-user TCC question); virtual display may or may not succeed in a non-console session (answers that question).
5. If Jump Desktop Connect supports multiple users on this Mac: connect as your user and as `rdpspike` from two Windows sessions simultaneously; while both are connected, run `launchctl print system | grep -i jump`, `ps -axo user,pid,command | grep -i jump`, and `sudo log show --last 5m --predicate 'process CONTAINS "Jump"' | grep -i -E 'session|virtual|display' | tail -50`, and record which process runs as which user and which session APIs appear.
6. Repeat step 5 with macOS Screen Sharing ("Ask to share" off; connect as `rdpspike` while your user is on console) and capture the same three commands with `screensharing` in place of `Jump`.

- [ ] **Step 6: Write the verdict**

Create `docs/research/2026-09-29-multisession-spike.md` (use the actual date) with these sections, each filled with evidence from Steps 2–5:
- **Question** (copy from the spike README)
- **How Jump Desktop Connect does it** (processes/users/launchd jobs/symbols observed)
- **How Screen Sharing does it** (same)
- **Probe results** — the console and background JSON lines verbatim
- **Per-user TCC** — whether grants had to be repeated for `rdpspike`, and the implication (first-run assistant per user vs MDM PPPC profile)
- **Verdict** — the mechanism Phase 3 will use for simultaneous sessions, or the reason to fall back to Fast User Switching, and the private symbols Phase 3 depends on
- **Next** — concrete Phase 3 design deltas, if any

- [ ] **Step 7: Clean up and commit**

```bash
sudo rm -f /Users/Shared/probe
git add spikes/multisession docs/research/*multisession-spike.md
git commit -m "spike(multisession): probe + verdict on simultaneous macOS sessions (throwaway)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```
The `rdpspike` account can be deleted by the user afterwards (System Settings → Users & Groups).

---

### Task 11: Apple entitlement requests (user submits)

**Files:**
- Create: `docs/product/entitlements.md`

**Interfaces:**
- Produces: a checklist of entitlements, where each is requested, and submission status — consumed by Phase 4 (USB, FSKit, camera) and Phase 5 (packaging).

- [ ] **Step 1: Write the checklist**

Create `docs/product/entitlements.md`:

```markdown
# Apple entitlements and capabilities

Team ID: (fill in when the account is confirmed)   Account holder: nazmolla

| Entitlement / capability | Needed for | How obtained | Status | Date |
|---|---|---|---|---|
| Developer ID Application + Installer certificates | Signing/notarizing app and .pkg (spec §13) | developer.apple.com → Certificates | not started | |
| `com.apple.developer.usb.host-controller-interface` | Generic USB redirection (spec §10.2) | Feedback Assistant request (managed); text: upstream `docs/entitlement-request.md` with our Team ID and product description | not submitted | |
| `com.apple.developer.system-extension.install` | Virtual camera system extension | Capability on the App ID | not started | |
| FSKit module capability | Drive redirection backend (spec §10.2) | Capability on the extension's App ID | not started | |

Private APIs (CGVirtualDisplay, session APIs from the spike) need no entitlement
but exclude Mac App Store distribution (spec §13).
```

- [ ] **Step 2: User submits**

The user (Apple Developer account holder) must: confirm or create the Apple Developer Program membership, fill in the Team ID above, create the two Developer ID certificates, and submit the USB host-controller entitlement request via Feedback Assistant using the upstream draft text (replace macrdp-specific wording with this product's description). Update each row's Status and Date.

- [ ] **Step 3: Commit**

```bash
git add docs/product/entitlements.md
git commit -m "docs(product): Apple entitlement checklist and submission status

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

## Phase 0 exit checklist
- [ ] `cargo test --locked` passes; Linux CI green on `main`.
- [ ] `docs/research/phase0-baseline.md` has build/test, color, host-perf and client/Jump sections with no empty cells.
- [ ] `docs/research/*-multisession-spike.md` states the Phase 3 mechanism (or the fallback and why).
- [ ] `docs/product/entitlements.md` shows the USB request submitted.
