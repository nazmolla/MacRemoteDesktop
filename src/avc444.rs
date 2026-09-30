//! AVC444 YUV plane split/combine helpers ([MS-RDPEGFX] §3.3.8.3.2, v1).
//!
//! The AVC444 server sends *two* H.264-encoded YUV420 frames per video frame: a
//! "main" view carrying full-resolution luma plus a 4:2:0 chroma subsample, and
//! an "auxiliary" view carrying the chroma samples that 4:2:0 subsampling
//! dropped. The receiver decodes both as YUV420, then merges them into a single
//! full-resolution YUV444 frame for display.
//!
//! - [`split_yuv444_to_yuv420_v1`] does the *encoder* side — full-res YUV444 →
//!   main + auxiliary YUV420.
//! - [`combine_luma_to_yuv444`] / [`combine_chroma_v1_to_yuv444`] do the
//!   *decoder* side. Both must be applied (luma first, then chroma) to fully
//!   reconstruct YUV444.
//!
//! The combine functions are ported essentially verbatim from FreeRDP's
//! `general_LumaToYUV444` / `general_ChromaV1ToYUV444`
//! (`libfreerdp/primitives/prim_YUV.c`, Apache-2.0); they're the canonical
//! reference because every real AVC444 client in the wild today is FreeRDP- or
//! mstsc-based and decodes against the same scheme. The split is *not* a
//! verbatim port: FreeRDP's `general_YUV444SplitToYUV420` swaps U and V in the
//! main view's chroma (writes `pU[x] = pSrcV[2*x]`) which contradicts both the
//! §3.3.8.3.2 pseudo-code and FreeRDP's own combine, and is unreachable through
//! their normal RGB→AVC444YUV server path. This implementation follows the spec
//! pseudo-code instead, verified by the roundtrip test below.
//!
//! Encoder-side input: [`bgra_to_yuv444_full_bt709`] converts a captured BGRA
//! frame to full-range BT.709 YUV444 (the same matrix `color_metrics` uses to
//! interpret decoded output), and [`aligned_dims`] + [`pad_plane_edge`] bring
//! an arbitrary true size (e.g. 1714×1287) up to the 16-aligned size the H.264
//! subframes are encoded at; see [`pad_plane_edge`] for how the caller crops.
//!
//! Buffer-size contract:
//! - **Width and height MUST be even.** [MS-RDPEGFX] §2.2.4.4 requires
//!   multiples of 16 on the H.264 wire (which implies even); the math here
//!   only needs even, but production callers should always pad to 16.
//! - Main view: Y = `width × height`, U/V = `(width / 2) × (height / 2)`.
//! - Auxiliary view: Y = `width × padded_aux_height(height)` (height is padded
//!   to a multiple of 16; the §3.3.8.3.2 B4/B5 packing uses 8-line strips and
//!   spec/FreeRDP both overpad), U/V = same as main.
//! - All planes are tightly packed with `stride = plane_width` in the test
//!   helpers; the production API takes explicit strides.

#![allow(dead_code)] // wired into the h264 pipeline in a follow-up commit

/// Full-range BT.709 BGRA → YUV444 in 16.16 fixed point, matching
/// `color_metrics::rgb8_to_yuv709_full` (Y = 0.2126 R + 0.7152 G + 0.0722 B,
/// Cb = (B − Y) / 1.8556 + 128, Cr = (R − Y) / 1.5748 + 128). Each row of
/// coefficients sums to exactly 65536 (Y) or 0 (Cb, Cr), so greys map to
/// Cb = Cr = 128 with no drift.
const Y_R: i32 = 13_933;
const Y_G: i32 = 46_871;
const Y_B: i32 = 4_732;
const CB_R: i32 = -7_509;
const CB_G: i32 = -25_259;
const CB_B: i32 = 32_768;
const CR_R: i32 = 32_768;
const CR_G: i32 = -29_763;
const CR_B: i32 = -3_005;
/// Rounding term (0.5 in 16.16).
const HALF: i32 = 1 << 15;
/// Chroma offset (128 in 16.16) plus rounding.
const CHROMA_BIAS: i32 = (128 << 16) + HALF;

/// Convert a BGRA frame (`stride` bytes per row, alpha ignored) to full-range
/// BT.709 YUV444 planes, tightly packed (`stride == width`). Allocates the three
/// output planes once per call and nothing per pixel; use
/// [`bgra_to_yuv444_full_bt709_into`] to reuse buffers across frames.
pub fn bgra_to_yuv444_full_bt709(
    bgra: &[u8],
    stride: usize,
    width: usize,
    height: usize,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; width * height];
    let mut u = vec![0u8; width * height];
    let mut v = vec![0u8; width * height];
    bgra_to_yuv444_full_bt709_into(bgra, stride, width, height, &mut y, &mut u, &mut v, width);
    (y, u, v)
}

/// [`bgra_to_yuv444_full_bt709`] into caller-owned planes of row stride
/// `dst_stride` (≥ `width`). Only the `width × height` area is written, so the
/// planes may be allocated at padded dimensions (see [`aligned_dims`]) and
/// completed with [`pad_plane_edge`].
///
/// The inner loop is branch-free integer arithmetic over `chunks_exact(4)`,
/// which LLVM auto-vectorizes.
#[allow(clippy::too_many_arguments)]
pub fn bgra_to_yuv444_full_bt709_into(
    bgra: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    dst_y: &mut [u8],
    dst_u: &mut [u8],
    dst_v: &mut [u8],
    dst_stride: usize,
) {
    assert!(
        stride >= width * 4,
        "BGRA stride {stride} < width {width} * 4"
    );
    assert!(
        dst_stride >= width,
        "plane stride {dst_stride} < width {width}"
    );
    assert!(
        bgra.len() >= stride * height.saturating_sub(1) + width * 4,
        "BGRA buffer too small for {width}x{height}"
    );
    let plane_len = dst_stride * height.saturating_sub(1) + width;
    assert!(
        dst_y.len() >= plane_len && dst_u.len() >= plane_len && dst_v.len() >= plane_len,
        "YUV444 planes too small for {width}x{height} at stride {dst_stride}"
    );
    for row in 0..height {
        let src = &bgra[row * stride..row * stride + width * 4];
        let o = row * dst_stride;
        let (ys, us, vs) = (
            &mut dst_y[o..o + width],
            &mut dst_u[o..o + width],
            &mut dst_v[o..o + width],
        );
        for (((px, y), u), v) in src.chunks_exact(4).zip(ys).zip(us).zip(vs) {
            let (b, g, r) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            // Y can't leave 0..=255 (non-negative weights summing to 1.0).
            *y = ((Y_R * r + Y_G * g + Y_B * b + HALF) >> 16) as u8;
            *u = ((CB_R * r + CB_G * g + CB_B * b + CHROMA_BIAS) >> 16).clamp(0, 255) as u8;
            *v = ((CR_R * r + CR_G * g + CR_B * b + CHROMA_BIAS) >> 16).clamp(0, 255) as u8;
        }
    }
}

/// Encode dimensions for a `width × height` frame: each rounded up to a
/// multiple of 16, which [MS-RDPEGFX] §2.2.4.4 requires of AVC420/AVC444
/// streams (and which makes both even, as the 4:2:0 split needs).
pub fn aligned_dims(width: usize, height: usize) -> (usize, usize) {
    (width.next_multiple_of(16), height.next_multiple_of(16))
}

/// Fill the padding of one plane by edge replication: columns
/// `width..padded_width` of every visible row take that row's last pixel, then
/// rows `height..padded_height` copy the last visible row. The plane must hold
/// `padded_height` rows of `stride ≥ padded_width` bytes, with the visible
/// `width × height` area already written.
///
/// Why replicate rather than pad with black: a hard edge next to the last
/// visible row/column costs bits and, in the 4:2:0 main view, a 2×2 chroma
/// block straddling the boundary (odd `width`/`height`) would mix padding into
/// the visible pixel's chroma. With replication that block holds copies of the
/// visible sample, so it stays exact.
///
/// How the caller crops: encode and split at `aligned_dims(w, h)`, but keep the
/// EGFX surface at the true `w × h` and send region rectangles (the AVC420
/// `regionRects` of each subframe) clipped to `w × h` — e.g. 1714×1287 is
/// encoded at 1728×1296 with a region of `0,0 .. 1713,1286` inclusive. The
/// client copies decoded pixels only inside the regions, so padding never
/// reaches the surface. Decoder-side tests crop the same way: combine at the
/// padded size, then read only the visible area.
pub fn pad_plane_edge(
    plane: &mut [u8],
    stride: usize,
    width: usize,
    height: usize,
    padded_width: usize,
    padded_height: usize,
) {
    assert!(width > 0 && height > 0, "empty visible area");
    assert!(padded_width >= width && padded_height >= height);
    assert!(
        stride >= padded_width,
        "stride {stride} < padded width {padded_width}"
    );
    assert!(
        plane.len() >= stride * (padded_height - 1) + padded_width,
        "plane too small for {padded_width}x{padded_height} at stride {stride}"
    );
    if padded_width > width {
        for row in 0..height {
            let o = row * stride;
            let last = plane[o + width - 1];
            plane[o + width..o + padded_width].fill(last);
        }
    }
    let last_row = (height - 1) * stride;
    for row in height..padded_height {
        plane.copy_within(last_row..last_row + padded_width, row * stride);
    }
}

/// Height the auxiliary Y plane must be allocated to. Matches FreeRDP's
/// `padHeigth = height + 16 - height % 16`. Adds 16 when already aligned, which
/// is intentional padding margin — B4/B5 packing only writes through `height`
/// odd rows, so the over-allocation is just unused space.
pub fn padded_aux_height(height: usize) -> usize {
    height - height % 16 + 16
}

/// Split a full-resolution YUV444 frame into main + auxiliary YUV420 frames per
/// [MS-RDPEGFX] §3.3.8.3.2 v1. All slices are tightly packed with the given
/// strides; the caller is responsible for sizing them per the contract above.
#[allow(clippy::too_many_arguments)]
pub fn split_yuv444_to_yuv420_v1(
    src_y: &[u8],
    src_u: &[u8],
    src_v: &[u8],
    src_stride_y: usize,
    src_stride_u: usize,
    src_stride_v: usize,
    main_y: &mut [u8],
    main_u: &mut [u8],
    main_v: &mut [u8],
    main_stride_y: usize,
    main_stride_u: usize,
    main_stride_v: usize,
    aux_y: &mut [u8],
    aux_u: &mut [u8],
    aux_v: &mut [u8],
    aux_stride_y: usize,
    aux_stride_u: usize,
    aux_stride_v: usize,
    width: usize,
    height: usize,
) {
    debug_assert!(
        width.is_multiple_of(2) && height.is_multiple_of(2),
        "dimensions must be even"
    );
    let half_width = width / 2;
    let half_height = height / 2;
    let pad_height = padded_aux_height(height);

    // B1: main Y = src Y, full resolution.
    for y in 0..height {
        let src_row = &src_y[y * src_stride_y..y * src_stride_y + width];
        let dst_row = &mut main_y[y * main_stride_y..y * main_stride_y + width];
        dst_row.copy_from_slice(src_row);
    }

    // B2, B3: main U/V = even-row even-column samples (the standard 4:2:0
    // subsample). Per §3.3.8.3.2 pseudo-code: B2[x,y] = U444[2x, 2y].
    for y in 0..half_height {
        let src_u_row = &src_u[2 * y * src_stride_u..2 * y * src_stride_u + width];
        let src_v_row = &src_v[2 * y * src_stride_v..2 * y * src_stride_v + width];
        let dst_u_row = &mut main_u[y * main_stride_u..y * main_stride_u + half_width];
        let dst_v_row = &mut main_v[y * main_stride_v..y * main_stride_v + half_width];
        for x in 0..half_width {
            dst_u_row[x] = src_u_row[2 * x];
            dst_v_row[x] = src_v_row[2 * x];
        }
    }

    // B4, B5: odd-row samples of U then V, interleaved into the auxiliary Y
    // plane in 8-line strips. Within each 16-line block: first 8 lines carry
    // odd U rows, next 8 lines carry odd V rows. The pad_height overshoot is
    // skipped via `pos >= height` checks (matches FreeRDP behaviour).
    let mut u_y = 0usize;
    let mut v_y = 0usize;
    for y in 0..pad_height {
        if (y % 16) < 8 {
            let pos = 2 * u_y + 1;
            u_y += 1;
            if pos >= height {
                continue;
            }
            let src_row = &src_u[pos * src_stride_u..pos * src_stride_u + width];
            let dst_row = &mut aux_y[y * aux_stride_y..y * aux_stride_y + width];
            dst_row.copy_from_slice(src_row);
        } else {
            let pos = 2 * v_y + 1;
            v_y += 1;
            if pos >= height {
                continue;
            }
            let src_row = &src_v[pos * src_stride_v..pos * src_stride_v + width];
            let dst_row = &mut aux_y[y * aux_stride_y..y * aux_stride_y + width];
            dst_row.copy_from_slice(src_row);
        }
    }

    // B6, B7: even-row odd-column samples of U and V → auxiliary U and V planes
    // (themselves 4:2:0-shaped). Per §3.3.8.3.2: B6[x,y] = U444[2x+1, 2y].
    for y in 0..half_height {
        let src_u_row = &src_u[2 * y * src_stride_u..2 * y * src_stride_u + width];
        let src_v_row = &src_v[2 * y * src_stride_v..2 * y * src_stride_v + width];
        let dst_u_row = &mut aux_u[y * aux_stride_u..y * aux_stride_u + half_width];
        let dst_v_row = &mut aux_v[y * aux_stride_v..y * aux_stride_v + half_width];
        for x in 0..half_width {
            // 2x+1 is the odd column; clamp to width-1 on the right edge so a
            // non-even width doesn't read past the source row.
            let col = (2 * x + 1).min(width - 1);
            dst_u_row[x] = src_u_row[col];
            dst_v_row[x] = src_v_row[col];
        }
    }
}

/// Decoder LUMA pass — ported from FreeRDP's `general_LumaToYUV444`. Copies the
/// main-view Y to dst Y at full resolution and replicates the main-view U/V
/// samples across every position of each 2x2 block. The chroma pass below
/// overwrites the odd positions with the auxiliary samples.
#[allow(clippy::too_many_arguments)]
pub fn combine_luma_to_yuv444(
    main_y: &[u8],
    main_u: &[u8],
    main_v: &[u8],
    main_stride_y: usize,
    main_stride_u: usize,
    main_stride_v: usize,
    dst_y: &mut [u8],
    dst_u: &mut [u8],
    dst_v: &mut [u8],
    dst_stride_y: usize,
    dst_stride_u: usize,
    dst_stride_v: usize,
    width: usize,
    height: usize,
) {
    debug_assert!(
        width.is_multiple_of(2) && height.is_multiple_of(2),
        "dimensions must be even"
    );
    let half_width = width / 2;
    let half_height = height / 2;

    // B1: dst Y = main Y.
    for y in 0..height {
        let src_row = &main_y[y * main_stride_y..y * main_stride_y + width];
        let dst_row = &mut dst_y[y * dst_stride_y..y * dst_stride_y + width];
        dst_row.copy_from_slice(src_row);
    }

    // B2, B3: replicate main U/V into the four positions of each 2x2 block.
    for y in 0..half_height {
        let src_u_row = &main_u[y * main_stride_u..y * main_stride_u + half_width];
        let src_v_row = &main_v[y * main_stride_v..y * main_stride_v + half_width];
        let val_2y = 2 * y;
        let val_2y1 = val_2y + 1;
        // Slice both rows we'll write (guarded by even-height path; odd height
        // skips the second).
        let (dst_u_top, mut dst_u_bot) =
            split_two_rows_mut(dst_u, dst_stride_u, val_2y, val_2y1, width);
        let (dst_v_top, mut dst_v_bot) =
            split_two_rows_mut(dst_v, dst_stride_v, val_2y, val_2y1, width);
        for x in 0..half_width {
            let val_2x = 2 * x;
            let val_2x1 = val_2x + 1;
            let u = src_u_row[x];
            let v = src_v_row[x];
            dst_u_top[val_2x] = u;
            dst_v_top[val_2x] = v;
            if val_2x1 < width {
                dst_u_top[val_2x1] = u;
                dst_v_top[val_2x1] = v;
            }
            if let (Some(du), Some(dv)) = (dst_u_bot.as_deref_mut(), dst_v_bot.as_deref_mut()) {
                du[val_2x] = u;
                dv[val_2x] = v;
                if val_2x1 < width {
                    du[val_2x1] = u;
                    dv[val_2x1] = v;
                }
            }
        }
    }
}

/// Decoder CHROMAv1 pass — ported from FreeRDP's `general_ChromaV1ToYUV444`.
/// Reverses [`split_yuv444_to_yuv420_v1`]'s B4/B5/B6/B7 packing back into the
/// dst U/V planes' odd positions.
#[allow(clippy::too_many_arguments)]
pub fn combine_chroma_v1_to_yuv444(
    aux_y: &[u8],
    aux_u: &[u8],
    aux_v: &[u8],
    aux_stride_y: usize,
    aux_stride_u: usize,
    aux_stride_v: usize,
    dst_u: &mut [u8],
    dst_v: &mut [u8],
    dst_stride_u: usize,
    dst_stride_v: usize,
    width: usize,
    height: usize,
) {
    let half_width = width / 2;
    let half_height = height / 2;
    let pad_height = padded_aux_height(height);

    // B4, B5: auxiliary Y plane carries odd U rows in 8-line strips, then
    // odd V rows in the next 8 lines, per 16-line block.
    let mut u_y = 0usize;
    let mut v_y = 0usize;
    for y in 0..pad_height {
        if (y % 16) < 8 {
            let pos = 2 * u_y + 1;
            u_y += 1;
            if pos >= height {
                continue;
            }
            let src_row = &aux_y[y * aux_stride_y..y * aux_stride_y + width];
            let dst_row = &mut dst_u[pos * dst_stride_u..pos * dst_stride_u + width];
            dst_row.copy_from_slice(src_row);
        } else {
            let pos = 2 * v_y + 1;
            v_y += 1;
            if pos >= height {
                continue;
            }
            let src_row = &aux_y[y * aux_stride_y..y * aux_stride_y + width];
            let dst_row = &mut dst_v[pos * dst_stride_v..pos * dst_stride_v + width];
            dst_row.copy_from_slice(src_row);
        }
    }

    // B6, B7: even-row odd-column samples. Writes 2x+1 of each even dst row.
    for y in 0..half_height {
        let src_u_row = &aux_u[y * aux_stride_u..y * aux_stride_u + half_width];
        let src_v_row = &aux_v[y * aux_stride_v..y * aux_stride_v + half_width];
        let val_2y = 2 * y;
        let dst_u_row = &mut dst_u[val_2y * dst_stride_u..val_2y * dst_stride_u + width];
        let dst_v_row = &mut dst_v[val_2y * dst_stride_v..val_2y * dst_stride_v + width];
        for x in 0..half_width {
            let val_2x1 = 2 * x + 1;
            dst_u_row[val_2x1] = src_u_row[x];
            dst_v_row[val_2x1] = src_v_row[x];
        }
    }
}

/// Borrow two disjoint rows from a strided plane. The bottom row is `None`
/// when it falls outside `height` (caller's responsibility to verify).
fn split_two_rows_mut(
    plane: &mut [u8],
    stride: usize,
    top: usize,
    bot: usize,
    row_len: usize,
) -> (&mut [u8], Option<&mut [u8]>) {
    let top_off = top * stride;
    let bot_off = bot * stride;
    if bot_off + row_len > plane.len() {
        return (&mut plane[top_off..top_off + row_len], None);
    }
    let (lo, hi) = plane.split_at_mut(bot_off);
    (
        &mut lo[top_off..top_off + row_len],
        Some(&mut hi[..row_len]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a deterministic YUV444 source using a per-plane seeded LCG so
    /// each pixel has a distinct value with no correlation to its neighbours.
    /// Width/height are arbitrary (any positive values, even or odd).
    fn synthetic_yuv444(width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut y = vec![0u8; width * height];
        let mut u = vec![0u8; width * height];
        let mut v = vec![0u8; width * height];
        let lcg = |seed: u64, n: usize| -> u8 {
            let mut s = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(n as u64);
            s ^= s >> 33;
            (s & 0xff) as u8
        };
        for j in 0..height {
            for i in 0..width {
                let idx = j * width + i;
                y[idx] = lcg(0x1111_1111, idx);
                u[idx] = lcg(0x2222_2222, idx);
                v[idx] = lcg(0x3333_3333, idx);
            }
        }
        (y, u, v)
    }

    /// End-to-end roundtrip: split full-res YUV444 → main + aux YUV420 →
    /// combine back to YUV444 → assert pixel-equality with the source.
    fn roundtrip(width: usize, height: usize) {
        let (src_y, src_u, src_v) = synthetic_yuv444(width, height);

        // Even dimensions enforced by the helper itself; matches the prod
        // contract (always padded to a multiple of 16).
        let half_w = width / 2;
        let half_h = height / 2;
        let pad_h = padded_aux_height(height);

        let mut main_y = vec![0u8; width * height];
        let mut main_u = vec![0u8; half_w * half_h];
        let mut main_v = vec![0u8; half_w * half_h];
        let mut aux_y = vec![0u8; width * pad_h];
        let mut aux_u = vec![0u8; half_w * half_h];
        let mut aux_v = vec![0u8; half_w * half_h];

        split_yuv444_to_yuv420_v1(
            &src_y,
            &src_u,
            &src_v,
            width,
            width,
            width,
            &mut main_y,
            &mut main_u,
            &mut main_v,
            width,
            half_w,
            half_w,
            &mut aux_y,
            &mut aux_u,
            &mut aux_v,
            width,
            half_w,
            half_w,
            width,
            height,
        );

        let mut dst_y = vec![0u8; width * height];
        let mut dst_u = vec![0u8; width * height];
        let mut dst_v = vec![0u8; width * height];

        combine_luma_to_yuv444(
            &main_y, &main_u, &main_v, width, half_w, half_w, &mut dst_y, &mut dst_u, &mut dst_v,
            width, width, width, width, height,
        );
        combine_chroma_v1_to_yuv444(
            &aux_y, &aux_u, &aux_v, width, half_w, half_w, &mut dst_u, &mut dst_v, width, width,
            width, height,
        );

        // Y is sent at full resolution; every byte must match.
        assert_eq!(dst_y, src_y, "Y plane mismatch ({width}x{height})");

        // U/V: every sample participated in the split — even-even via B2/B3,
        // even-odd via B6/B7, odd-* via B4/B5. The whole plane must roundtrip.
        for j in 0..height {
            for i in 0..width {
                let idx = j * width + i;
                assert_eq!(
                    dst_u[idx], src_u[idx],
                    "U mismatch at ({i},{j}) for {width}x{height}"
                );
                assert_eq!(
                    dst_v[idx], src_v[idx],
                    "V mismatch at ({i},{j}) for {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn roundtrip_16x16() {
        roundtrip(16, 16);
    }

    #[test]
    fn roundtrip_64x48() {
        roundtrip(64, 48);
    }

    #[test]
    fn roundtrip_1920x1080() {
        roundtrip(1920, 1080);
    }

    /// Even, non-16-aligned sizes. AVC444's wire contract requires 16-alignment
    /// so production never sees these (we pad), but the split/combine math
    /// only needs even dimensions.
    #[test]
    fn roundtrip_even_non_16_aligned() {
        roundtrip(18, 16);
        roundtrip(34, 32);
        roundtrip(100, 64);
        roundtrip(64, 100);
    }

    /// The real Mac display size (1920x1080) needs height-padding to 1088 to
    /// be spec-legal for AVC444. Cover both: native 1920x1080 (even, just
    /// not 16-aligned in height) and the 16-aligned padded form.
    #[test]
    fn roundtrip_realistic_display_sizes() {
        roundtrip(1920, 1080);
        roundtrip(1920, 1088);
        roundtrip(2560, 1440);
        roundtrip(3840, 2160);
    }

    /// The fixed-point conversion agrees with the float reference in
    /// `color_metrics` (the interpretation the decode side applies) to within
    /// one code value, and exactly on the overwhelming majority of inputs.
    #[test]
    fn bgra_to_yuv444_matches_float_bt709_reference() {
        use crate::color_metrics::rgb8_to_yuv709_full;
        let mut bgra = Vec::new();
        for r in (0..=255u8).step_by(5) {
            for g in (0..=255u8).step_by(5) {
                for b in (0..=255u8).step_by(5) {
                    bgra.extend_from_slice(&[b, g, r, 255]);
                }
            }
        }
        let n = bgra.len() / 4;
        let (y, u, v) = bgra_to_yuv444_full_bt709(&bgra, n * 4, n, 1);
        let mut exact = 0usize;
        for i in 0..n {
            let px = &bgra[i * 4..i * 4 + 4];
            let want = rgb8_to_yuv709_full([px[2], px[1], px[0]]);
            let got = [y[i], u[i], v[i]];
            for c in 0..3 {
                assert!(
                    want[c].abs_diff(got[c]) <= 1,
                    "rgb {:?}: fixed {got:?} vs float {want:?}",
                    [px[2], px[1], px[0]]
                );
            }
            exact += usize::from(want == got);
        }
        assert!(exact * 100 >= n * 99, "only {exact}/{n} exact");
        // Greys carry no chroma.
        let (_, u, v) = bgra_to_yuv444_full_bt709(&[0, 0, 0, 255, 255, 255, 255, 255], 8, 2, 1);
        assert_eq!((u, v), (vec![128, 128], vec![128, 128]));
    }

    #[test]
    fn aligned_dims_round_up_to_16() {
        assert_eq!(aligned_dims(1920, 1080), (1920, 1088));
        assert_eq!(aligned_dims(1714, 1287), (1728, 1296));
        assert_eq!(aligned_dims(16, 16), (16, 16));
        assert_eq!(aligned_dims(1, 1), (16, 16));
    }

    #[test]
    fn pad_plane_edge_replicates_last_column_and_row() {
        let (w, h, pw, ph, stride) = (5, 3, 16, 16, 20);
        let mut plane = vec![0u8; stride * ph];
        for row in 0..h {
            for col in 0..w {
                plane[row * stride + col] = (row * 10 + col) as u8;
            }
        }
        pad_plane_edge(&mut plane, stride, w, h, pw, ph);
        for row in 0..ph {
            for col in 0..pw {
                let want = (row.min(h - 1) * 10 + col.min(w - 1)) as u8;
                assert_eq!(plane[row * stride + col], want, "({col},{row})");
            }
            // Bytes past the padded width (stride slack) are left alone.
            assert!(plane[row * stride + pw..(row + 1) * stride]
                .iter()
                .all(|&b| b == 0));
        }
    }

    /// Worst per-channel error, over the visible `width × height`, of
    /// BGRA → YUV444 (padded to 16) → AVC444 v1 split → combine → RGB.
    /// Returns (max error anywhere, max error inside the chroma-edge strip).
    fn pattern_roundtrip_max_error(width: usize, height: usize, with_aux: bool) -> (u8, u8) {
        use crate::color_metrics::yuv709_full_to_rgb8;
        let p = crate::color_pattern::generate(width, height);
        let (pw, ph) = aligned_dims(width, height);
        let (hw, hh, aux_h) = (pw / 2, ph / 2, padded_aux_height(ph));

        let mut src = [vec![0u8; pw * ph], vec![0u8; pw * ph], vec![0u8; pw * ph]];
        let [sy, su, sv] = &mut src;
        bgra_to_yuv444_full_bt709_into(&p.bgra, width * 4, width, height, sy, su, sv, pw);
        for plane in [&mut *sy, &mut *su, &mut *sv] {
            pad_plane_edge(plane, pw, width, height, pw, ph);
        }

        let (mut my, mut mu, mut mv) = (vec![0u8; pw * ph], vec![0u8; hw * hh], vec![0u8; hw * hh]);
        let (mut ay, mut au, mut av) = (
            vec![0u8; pw * aux_h],
            vec![0u8; hw * hh],
            vec![0u8; hw * hh],
        );
        split_yuv444_to_yuv420_v1(
            sy, su, sv, pw, pw, pw, &mut my, &mut mu, &mut mv, pw, hw, hw, &mut ay, &mut au,
            &mut av, pw, hw, hw, pw, ph,
        );

        let (mut dy, mut du, mut dv) = (vec![0u8; pw * ph], vec![0u8; pw * ph], vec![0u8; pw * ph]);
        combine_luma_to_yuv444(
            &my, &mu, &mv, pw, hw, hw, &mut dy, &mut du, &mut dv, pw, pw, pw, pw, ph,
        );
        if with_aux {
            combine_chroma_v1_to_yuv444(
                &ay, &au, &av, pw, hw, hw, &mut du, &mut dv, pw, pw, pw, ph,
            );
        }

        let e = p.edge;
        let (mut worst, mut worst_edge) = (0u8, 0u8);
        // Crop: only the visible area is compared (the padding never reaches
        // the client surface).
        for row in 0..height {
            for col in 0..width {
                let i = row * pw + col;
                let got = yuv709_full_to_rgb8(dy[i], du[i], dv[i]);
                let s = (row * width + col) * 4;
                let want = [p.bgra[s + 2], p.bgra[s + 1], p.bgra[s]];
                let err = (0..3).map(|c| want[c].abs_diff(got[c])).max().unwrap_or(0);
                worst = worst.max(err);
                if (e.x..e.x + e.w).contains(&col) && (e.y..e.y + e.h).contains(&row) {
                    worst_edge = worst_edge.max(err);
                }
            }
        }
        (worst, worst_edge)
    }

    /// Phase 2a colour acceptance for the AVC444 math (no codec in the loop):
    /// the ColorChecker patches, the background and the 1-px red/blue
    /// chroma-edge strip all come back within ±1 per channel. The edge strip is
    /// the case 4:2:0 destroys (ΔE ≈ 32); 4:4:4 must keep it.
    #[test]
    fn colorchecker_roundtrip_via_v1_split_within_one_1920x1080() {
        assert_eq!(pattern_roundtrip_max_error(1920, 1080, true), (1, 1));
    }

    /// Odd true height (and a width that is even but not 16-aligned): padded to
    /// 1728×1296, cropped back to 1714×1287 — the last row must survive.
    #[test]
    fn colorchecker_roundtrip_via_v1_split_within_one_1714x1287_padded() {
        let (worst, worst_edge) = pattern_roundtrip_max_error(1714, 1287, true);
        assert!(
            worst <= 1 && worst_edge <= 1,
            "worst {worst}, edge strip {worst_edge}"
        );
    }

    /// Negative control: dropping the auxiliary view (i.e. plain 4:2:0) must
    /// fail the edge strip badly, so the ±1 assertions above have teeth.
    #[test]
    fn main_view_alone_fails_the_chroma_edge_strip() {
        let (_, worst_edge) = pattern_roundtrip_max_error(1714, 1287, false);
        assert!(
            worst_edge > 64,
            "4:2:0-only edge error unexpectedly small: {worst_edge}"
        );
    }

    #[test]
    fn padded_aux_height_examples() {
        // Multiples of 16 still add 16 (FreeRDP defensive overpad).
        assert_eq!(padded_aux_height(16), 32);
        assert_eq!(padded_aux_height(64), 80);
        // Non-aligned rounds up to next multiple of 16.
        assert_eq!(padded_aux_height(60), 64);
        assert_eq!(padded_aux_height(1), 16);
        assert_eq!(padded_aux_height(17), 32);
    }
}
