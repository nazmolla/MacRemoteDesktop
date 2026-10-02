//! BGRA → full-range BT.709 NV12 on the GPU, for the H.264 encoder.
//!
//! The Metal kernel in `gpu_convert.m` ports `bgra_to_nv12_full_range`
//! (src/videotoolbox.rs) exactly, so the colour result is unchanged; it only
//! moves the per-frame conversion, Viga's main CPU cost, off the CPU.
//! Best effort: any failure returns `false` and the caller converts on the
//! CPU. `MACRDP_GPU_CONVERT=0` turns it off.

use std::ffi::c_void;
use std::sync::OnceLock;

extern "C" {
    fn macrdp_gpu_nv12_attributes() -> *const c_void;
    fn macrdp_gpu_bgra_to_nv12(
        bgra: *const u8,
        stride: usize,
        width: u32,
        height: u32,
        dst: *const c_void,
    ) -> i32;
    fn macrdp_gpu_create_nv12(width: u32, height: u32) -> *const c_void;
    fn macrdp_gpu_convert_surface(src: *const c_void, dst: *const c_void) -> i32;
    #[cfg(test)]
    fn macrdp_gpu_selftest_surface(
        bgra: *const u8,
        width: u32,
        height: u32,
        y_out: *mut u8,
        cbcr_out: *mut u8,
    ) -> i32;
    #[cfg(test)]
    fn macrdp_gpu_selftest(
        bgra: *const u8,
        stride: usize,
        width: u32,
        height: u32,
        y_out: *mut u8,
        cbcr_out: *mut u8,
    ) -> i32;
}

/// A GPU-writable full-range NV12 buffer from a reusable pool (+1 retained; the
/// caller releases it), or `None` when the GPU path is off or unavailable.
pub fn create_nv12(width: u32, height: u32) -> Option<*const c_void> {
    nv12_attributes()?;
    // SAFETY: plain values in; returns a +1 CVPixelBuffer or NULL.
    let pb = unsafe { macrdp_gpu_create_nv12(width, height) };
    (!pb.is_null()).then_some(pb)
}

/// Convert the capture buffer `src` (32BGRA) into `dst` without copying it.
/// `false` = not done (sizes or formats differ, or the GPU failed).
///
/// # Safety
/// `src` and `dst` must be valid CVPixelBuffers for the duration of the call.
pub unsafe fn convert_surface(src: *const c_void, dst: *const c_void) -> bool {
    // SAFETY: both buffers are valid per the caller; the call waits for the GPU.
    unsafe { macrdp_gpu_convert_surface(src, dst) == 0 }
}

/// Test hook for the zero-copy path: same output as [`selftest`].
#[cfg(test)]
pub fn selftest_surface(bgra: &[u8], width: u32, height: u32) -> Option<(Vec<u8>, Vec<u8>)> {
    let (w, h) = (width as usize, height as usize);
    if bgra.len() < w * 4 * h {
        return None;
    }
    let mut y = vec![0u8; w * h];
    let mut c = vec![0u8; w * h / 2];
    // SAFETY: `bgra` holds `h` tight rows of `w * 4` bytes; the outputs hold w*h and w*h/2 bytes.
    let rc = unsafe {
        macrdp_gpu_selftest_surface(bgra.as_ptr(), width, height, y.as_mut_ptr(), c.as_mut_ptr())
    };
    (rc == 0).then_some((y, c))
}

/// Test hook: GPU-convert `bgra` and return the packed (Y, CbCr) planes.
#[cfg(test)]
pub fn selftest(bgra: &[u8], width: u32, height: u32) -> Option<(Vec<u8>, Vec<u8>)> {
    let (w, h) = (width as usize, height as usize);
    if bgra.len() < w * 4 * h {
        return None;
    }
    let mut y = vec![0u8; w * h];
    let mut c = vec![0u8; w * h / 2];
    // SAFETY: `bgra` holds `h` rows of `w * 4` bytes; the outputs hold w*h and w*h/2 bytes,
    // exactly what the hook writes.
    let rc = unsafe {
        macrdp_gpu_selftest(
            bgra.as_ptr(),
            w * 4,
            width,
            height,
            y.as_mut_ptr(),
            c.as_mut_ptr(),
        )
    };
    (rc == 0).then_some((y, c))
}

struct Attrs(*const c_void);
// SAFETY: the pointer is an immutable CFDictionary that is retained for the
// life of the process and only ever read.
unsafe impl Send for Attrs {}
// SAFETY: as above; CFDictionary is safe to read from any thread.
unsafe impl Sync for Attrs {}

/// Pixel-buffer attributes that make an NV12 buffer GPU-writable, or `None`
/// when the GPU path is off or Metal is unavailable.
pub fn nv12_attributes() -> Option<*const c_void> {
    static ATTRS: OnceLock<Attrs> = OnceLock::new();
    if crate::tunables::var("MACRDP_GPU_CONVERT").as_deref() == Ok("0") {
        return None;
    }
    // SAFETY: takes no arguments; returns a +1 CFDictionary (kept for the
    // process lifetime) or NULL.
    let a = ATTRS.get_or_init(|| Attrs(unsafe { macrdp_gpu_nv12_attributes() }));
    (!a.0.is_null()).then_some(a.0)
}

/// Convert `bgra` (`height` rows `stride` bytes apart) into `dst`, an even-sized
/// 420f buffer created with [`nv12_attributes`]. `false` = not done.
///
/// # Safety
/// `dst` must be a valid CVPixelBuffer of exactly `width × height`.
pub unsafe fn bgra_to_nv12(
    bgra: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    dst: *const c_void,
) -> bool {
    let Some(need) = stride.checked_mul(height as usize) else {
        return false;
    };
    if bgra.len() < need || stride < width as usize * 4 {
        return false;
    }
    // SAFETY: `bgra` holds at least `stride * height` bytes (checked above) and
    // outlives the call, which waits for the GPU; `dst` is valid per the caller.
    unsafe { macrdp_gpu_bgra_to_nv12(bgra.as_ptr(), stride, width, height, dst) == 0 }
}
