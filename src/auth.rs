//! Validate (username, password) against the macOS account database via PAM.
//!
//! We use the `checkpw` PAM service (auth + account, no session) so we don't
//! need root or special entitlements. The expected `username` is whatever the
//! user provided on the command line or defaulted to via `whoami`.
//!
//! On success the returned password is the verified Mac password; we hand it
//! to `ironrdp_server::RdpServer::set_credentials` so the per-connection
//! `ClientInfoPdu` comparison passes for clients that supply the same creds.

#[cfg(target_os = "macos")]
pub fn check(username: &str, password: &str) -> Verdict {
    pam_impl::check("checkpw", username, password)
}

#[cfg(not(target_os = "macos"))]
pub fn check(_username: &str, _password: &str) -> Verdict {
    // On non-macOS targets we don't gate at startup; the protocol layer is
    // the only thing we can compile-test there.
    Verdict::Accepted
}

/// The result of checking a password against the local account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The password is correct and the account may log in.
    Accepted,
    /// PAM refused: wrong password, unknown user, or a disabled, expired or
    /// locked account. The message is PAM's own description.
    Rejected(String),
    /// PAM could not give an answer (module or system error). Says nothing
    /// about the password.
    Unavailable(String),
}

/// Check the password once at startup; any answer other than
/// [`Verdict::Accepted`] is an error.
pub fn authenticate(username: &str, password: &str) -> anyhow::Result<()> {
    match check(username, password) {
        Verdict::Accepted => Ok(()),
        Verdict::Rejected(msg) | Verdict::Unavailable(msg) => {
            Err(anyhow::anyhow!("authentication failed: {msg}"))
        }
    }
}

#[cfg(target_os = "macos")]
mod pam_impl {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    use std::ptr;

    use super::Verdict;

    // libpam typedefs (see /usr/include/pam/pam_appl.h on macOS).
    #[repr(C)]
    struct PamMessage {
        msg_style: c_int,
        msg: *const c_char,
    }

    #[repr(C)]
    struct PamResponse {
        resp: *mut c_char,
        resp_retcode: c_int,
    }

    #[repr(C)]
    struct PamConv {
        conv: extern "C" fn(
            num_msg: c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
        appdata_ptr: *mut c_void,
    }

    const PAM_SUCCESS: c_int = 0;
    // OpenPAM result codes that mean "this password or account may not log
    // in" (openpam's pam_constants.h). Anything else that is not
    // PAM_SUCCESS is a failure to reach an answer.
    const PAM_PERM_DENIED: c_int = 7;
    const PAM_MAXTRIES: c_int = 8;
    const PAM_AUTH_ERR: c_int = 9;
    const PAM_NEW_AUTHTOK_REQD: c_int = 10;
    const PAM_USER_UNKNOWN: c_int = 13;
    const PAM_CRED_EXPIRED: c_int = 15;
    const PAM_ACCT_EXPIRED: c_int = 17;
    const PAM_AUTHTOK_EXPIRED: c_int = 18;

    fn is_rejection(rc: c_int) -> bool {
        matches!(
            rc,
            PAM_PERM_DENIED
                | PAM_MAXTRIES
                | PAM_AUTH_ERR
                | PAM_NEW_AUTHTOK_REQD
                | PAM_USER_UNKNOWN
                | PAM_CRED_EXPIRED
                | PAM_ACCT_EXPIRED
                | PAM_AUTHTOK_EXPIRED
        )
    }
    // Asks for the password (echo off). Used to know when to return the
    // stored password as the response.
    const PAM_PROMPT_ECHO_OFF: c_int = 1;
    // pam_set_item key for the password (PAM_AUTHTOK = 6 on macOS / OpenPAM).
    // /etc/pam.d/checkpw uses `use_first_pass`, so pam_opendirectory expects
    // the authtok already set on the handle and never calls our conv.
    const PAM_AUTHTOK: c_int = 6;

    #[link(name = "pam")]
    extern "C" {
        fn pam_start(
            service: *const c_char,
            user: *const c_char,
            conv: *const PamConv,
            handle: *mut *mut c_void,
        ) -> c_int;
        fn pam_set_item(handle: *mut c_void, item_type: c_int, item: *const c_void) -> c_int;
        fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;
        fn pam_acct_mgmt(handle: *mut c_void, flags: c_int) -> c_int;
        fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
        fn pam_strerror(handle: *mut c_void, errnum: c_int) -> *const c_char;
    }

    /// PAM conversation callback. The `checkpw` service uses `use_first_pass`,
    /// so pam_opendirectory normally reads the password from PAM_AUTHTOK
    /// (set in `authenticate`) and never invokes this — but other PAM services
    /// we might switch to do prompt, so we keep it real: malloc one
    /// `PamResponse` per message, copying our stored password (`appdata_ptr`
    /// points at a heap-allocated CString). libpam frees both the array and
    /// each `resp` buffer.
    extern "C" fn conv(
        num_msg: c_int,
        msgs: *mut *const PamMessage,
        out_resp: *mut *mut PamResponse,
        appdata_ptr: *mut c_void,
    ) -> c_int {
        if num_msg <= 0 || msgs.is_null() || out_resp.is_null() {
            return 1; // PAM_CONV_ERR
        }
        // SAFETY: calloc with a positive count (checked above) and the element size; the result is
        // checked for null before use.
        let arr = unsafe { libc::calloc(num_msg as usize, std::mem::size_of::<PamResponse>()) }
            as *mut PamResponse;
        if arr.is_null() {
            return 1;
        }
        // SAFETY: libpam passes back the `appdata_ptr` set in `check`, which points at the
        // `CString` that lives on `check`'s stack for the whole PAM transaction.
        let pw_cstr = unsafe { &*(appdata_ptr as *const CString) };
        for i in 0..(num_msg as isize) {
            // SAFETY: `msgs` is non-null (checked above) and libpam provides `num_msg` entries; `i`
            // is below `num_msg`.
            let m = unsafe { *msgs.offset(i) };
            if m.is_null() {
                continue;
            }
            // SAFETY: `m` is a non-null message pointer from libpam's array, valid for this call.
            let style = unsafe { (*m).msg_style };
            if style == PAM_PROMPT_ECHO_OFF {
                let bytes = pw_cstr.as_bytes_with_nul();
                // SAFETY: malloc of the password length plus NUL; the result is checked for null
                // before use.
                let buf = unsafe { libc::malloc(bytes.len()) } as *mut c_char;
                if buf.is_null() {
                    // SAFETY: `arr` holds `num_msg` zero-initialised entries
                    // (calloc) and only entries before `i` can have been filled.
                    unsafe { free_responses(arr, i) };
                    return 1;
                }
                // SAFETY: `buf` has room for `bytes.len()` bytes (just allocated), the source is
                // the CString's own bytes, the regions do not overlap, and `arr` has `num_msg`
                // entries with `i` below it.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr() as *const c_char,
                        buf,
                        bytes.len(),
                    );
                    (*arr.offset(i)).resp = buf;
                    (*arr.offset(i)).resp_retcode = 0;
                }
            }
        }
        // SAFETY: `out_resp` is non-null (checked above); ownership of `arr` passes to libpam,
        // which frees it.
        unsafe { *out_resp = arr };
        PAM_SUCCESS
    }

    /// Wipe and free the password copies in the first `filled` responses, then
    /// the array itself. Used when the conversation fails part-way, so libpam
    /// never sees (or frees) the array.
    ///
    /// # Safety
    /// `arr` must come from `calloc` with at least `filled` entries, each
    /// `resp` either null or a NUL-terminated buffer from `malloc`.
    unsafe fn free_responses(arr: *mut PamResponse, filled: isize) {
        for j in 0..filled {
            // SAFETY: j < filled, within the allocation (see above).
            let resp = unsafe { (*arr.offset(j)).resp };
            if resp.is_null() {
                continue;
            }
            // SAFETY: `resp` is a NUL-terminated malloc'd buffer we wrote.
            let len = unsafe { libc::strlen(resp) };
            for k in 0..len {
                // Volatile so the wipe is not optimised away before free.
                // SAFETY: k < len, inside the buffer.
                unsafe { std::ptr::write_volatile(resp.add(k), 0) };
            }
            // SAFETY: allocated with malloc in `conv`, freed exactly once.
            unsafe { libc::free(resp as *mut c_void) };
        }
        // SAFETY: allocated with calloc in `conv`, freed exactly once.
        unsafe { libc::free(arr as *mut c_void) };
    }

    pub fn check(service: &str, username: &str, password: &str) -> Verdict {
        use zeroize::Zeroizing;

        let Ok(service_c) = CString::new(service) else {
            return Verdict::Unavailable("service contains NUL".into());
        };
        let Ok(user_c) = CString::new(username) else {
            return Verdict::Rejected("username contains NUL".into());
        };
        let Ok(pw_c) = CString::new(password) else {
            return Verdict::Rejected("password contains NUL".into());
        };

        let conv_struct = PamConv {
            conv,
            appdata_ptr: &pw_c as *const _ as *mut c_void,
        };

        let mut handle: *mut c_void = ptr::null_mut();
        // SAFETY: all three strings are NUL-terminated CStrings that outlive the PAM handle,
        // `conv_struct` lives until pam_end below, and `handle` is a valid out-pointer.
        let rc = unsafe {
            pam_start(
                service_c.as_ptr(),
                user_c.as_ptr(),
                &conv_struct,
                &mut handle,
            )
        };
        if rc != PAM_SUCCESS {
            return Verdict::Unavailable(format!("pam_start failed: rc={rc}"));
        }

        // SAFETY: `handle` came from a successful pam_start; `pw_c` stays alive until after
        // pam_end, and OpenPAM copies the item anyway.
        let set_rc = unsafe { pam_set_item(handle, PAM_AUTHTOK, pw_c.as_ptr() as *const c_void) };
        if set_rc != PAM_SUCCESS {
            // SAFETY: `handle` came from a successful pam_start and is ended exactly once on this
            // path.
            unsafe { pam_end(handle, set_rc) };
            return Verdict::Unavailable(format!("pam_set_item(AUTHTOK) failed: rc={set_rc}"));
        }

        // SAFETY: `handle` came from a successful pam_start and has not been ended.
        let auth_rc = unsafe { pam_authenticate(handle, 0) };
        let acct_rc = if auth_rc == PAM_SUCCESS {
            // SAFETY: `handle` came from a successful pam_start and has not been ended.
            unsafe { pam_acct_mgmt(handle, 0) }
        } else {
            auth_rc
        };

        let err = if acct_rc != PAM_SUCCESS {
            // SAFETY: `handle` came from a successful pam_start and has not been ended; the
            // returned string is static PAM text.
            let msg_ptr = unsafe { pam_strerror(handle, acct_rc) };
            let msg = if msg_ptr.is_null() {
                "unknown PAM error".to_string()
            } else {
                // SAFETY: `msg_ptr` is non-null and points at a NUL-terminated message owned by
                // PAM, copied out before pam_end.
                unsafe { CStr::from_ptr(msg_ptr) }
                    .to_string_lossy()
                    .into_owned()
            };
            Some(format!("{msg} (rc={acct_rc})"))
        } else {
            None
        };

        // SAFETY: `handle` came from a successful pam_start and is ended exactly once, after its
        // last use.
        unsafe { pam_end(handle, acct_rc) };

        // Overwrite the password buffer before drop. CString's Drop frees
        // but doesn't zero; converting into its underlying Vec<u8> and
        // wrapping with Zeroizing lets the bytes be wiped at scope exit.
        // Safe ordering: pam_end has already returned, so libpam no longer
        // holds the pointer set via pam_set_item.
        let _wiped_pw = Zeroizing::new(pw_c.into_bytes_with_nul());

        match err {
            None => Verdict::Accepted,
            Some(msg) if is_rejection(acct_rc) => Verdict::Rejected(msg),
            Some(msg) => Verdict::Unavailable(msg),
        }
    }
}
