//! Application state and the small C interface consumed by the native WinUI app.
//! Network and capture work never runs on the UI thread.

mod engine;
pub mod model;

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

pub use engine::Engine;

#[no_mangle]
pub extern "C" fn airloom_create() -> *mut c_void {
    catch_unwind(|| match Engine::start() {
        Ok(engine) => Box::into_raw(Box::new(engine)).cast(),
        Err(_) => std::ptr::null_mut(),
    })
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `handle` must be null or the live pointer returned by `airloom_create`. The
/// caller must stop all concurrent calls before destroying it, exactly once.
#[no_mangle]
pub unsafe extern "C" fn airloom_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { Box::from_raw(handle.cast::<Engine>()) });
        }));
    }
}

/// # Safety
/// `handle` must be live; `json` must point to a NUL-terminated UTF-8 string valid
/// for the duration of this call. Authentication input stays in transient
/// command/authentication state and is never included in snapshots or logs.
/// The caller retains ownership of the input buffer and should clear secrets
/// from that buffer after the call. Auth responses must include the prompt's
/// `request_id`; an old dialog cannot authorize a newer authentication attempt.
#[no_mangle]
pub unsafe extern "C" fn airloom_command(handle: *mut c_void, json: *const c_char) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() || json.is_null() {
            return -1;
        }
        let bytes = unsafe { CStr::from_ptr(json) }.to_bytes();
        if bytes.len() > 8192 {
            return -1;
        }
        let Ok(command) = serde_json::from_slice::<model::Command>(bytes) else {
            return -1;
        };
        if !command.validate() {
            return -1;
        }
        match unsafe { &*handle.cast::<Engine>() }.send(command) {
            Ok(()) => 0,
            Err(_) => -2,
        }
    }))
    .unwrap_or(-2)
}

/// # Safety
/// `handle` must be a live handle or null. The returned string is owned by the
/// caller and must be released exactly once with `airloom_free_string`.
#[no_mangle]
pub unsafe extern "C" fn airloom_snapshot(handle: *mut c_void) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return std::ptr::null_mut();
        }
        let snapshot = unsafe { &*handle.cast::<Engine>() }.snapshot();
        serde_json::to_string(&snapshot)
            .ok()
            .and_then(|json| CString::new(json).ok())
            .map_or(std::ptr::null_mut(), CString::into_raw)
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `value` must be null or an unreleased pointer returned by `airloom_snapshot`.
#[no_mangle]
pub unsafe extern "C" fn airloom_free_string(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_abi_calls_are_safe_and_fail_explicitly() {
        unsafe {
            assert_eq!(airloom_command(std::ptr::null_mut(), std::ptr::null()), -1);
            assert!(airloom_snapshot(std::ptr::null_mut()).is_null());
            airloom_free_string(std::ptr::null_mut());
            airloom_destroy(std::ptr::null_mut());
        }
    }
}
