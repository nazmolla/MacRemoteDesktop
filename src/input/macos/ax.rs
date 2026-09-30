//! The Accessibility and Core Foundation calls the switcher, focus and window
//! helpers share.

pub(super) const AX_ERROR_SUCCESS: i32 = 0;
pub(super) const AX_ERROR_ILLEGAL_ARGUMENT: i32 = -25201; // close enough — only used for null guard

// Shared AX FFI surface used by the activation/window-cycling
// helpers below. macrdp already holds the Accessibility TCC grant
// (CGEventPost wouldn't work without it), so these calls don't
// trip a permission gate. All return `AXError`:
//   0       = kAXErrorSuccess
//   -25201  = kAXErrorAPIDisabled (AX permission missing)
//   -25204  = kAXErrorAttributeUnsupported (target has no AX
//             attribute — e.g., a freshly-launched app whose main
//             run loop hasn't installed AX yet)
//   -25205  = kAXErrorNotImplemented
//   -25211  = kAXErrorIllegalArgument (bad PID / null pointer)
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub(super) fn AXUIElementCreateApplication(pid: libc::pid_t) -> *mut std::ffi::c_void;
    pub(super) fn AXUIElementCreateSystemWide() -> *mut std::ffi::c_void;
    pub(super) fn AXUIElementGetPid(element: *mut std::ffi::c_void, pid: *mut libc::pid_t) -> i32;
    pub(super) fn AXUIElementCopyElementAtPosition(
        application: *mut std::ffi::c_void,
        x: f32,
        y: f32,
        element: *mut *mut std::ffi::c_void,
    ) -> i32;
    pub(super) fn AXUIElementSetAttributeValue(
        element: *mut std::ffi::c_void,
        attribute: core_foundation::base::CFTypeRef,
        value: core_foundation::base::CFTypeRef,
    ) -> i32;
    pub(super) fn AXUIElementCopyAttributeValue(
        element: *mut std::ffi::c_void,
        attribute: core_foundation::base::CFTypeRef,
        value: *mut core_foundation::base::CFTypeRef,
    ) -> i32;
    pub(super) fn AXUIElementPerformAction(
        element: *mut std::ffi::c_void,
        action: core_foundation::base::CFTypeRef,
    ) -> i32;
    pub(super) fn AXValueGetValue(
        value: core_foundation::base::CFTypeRef,
        the_type: u32,
        value_ptr: *mut std::ffi::c_void,
    ) -> u8;
    pub(super) fn AXValueCreate(
        the_type: u32,
        value_ptr: *const std::ffi::c_void,
    ) -> core_foundation::base::CFTypeRef;
    pub(super) fn CFRelease(cf: *const std::ffi::c_void);
    pub(super) fn CFEqual(a: *const std::ffi::c_void, b: *const std::ffi::c_void) -> u8;
    pub(super) fn CFBooleanGetValue(boolean: core_foundation::base::CFTypeRef) -> u8;
    pub(super) fn CFArrayGetCount(arr: *const std::ffi::c_void) -> isize;
    pub(super) fn CFArrayGetValueAtIndex(
        arr: *const std::ffi::c_void,
        idx: isize,
    ) -> *const std::ffi::c_void;
}
