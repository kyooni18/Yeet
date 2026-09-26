//! C ABI for embedding the frontend-neutral Yeet harness in native hosts.
//!
//! Commands and events retain the exact serde JSON representation used by the
//! Rust harness, while ownership and runtime execution stay in-process.

use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char},
    ptr,
};

use crate::harness::{Harness, HarnessCommand};

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_error(error: impl ToString) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(error.to_string());
    });
}

fn clear_error() {
    LAST_ERROR.with(|slot| {
        slot.borrow_mut().take();
    });
}

fn string_to_raw(value: String) -> *mut c_char {
    let sanitized = value.replace('\0', "\u{FFFD}");
    CString::new(sanitized)
        .expect("sanitized FFI string")
        .into_raw()
}

unsafe fn string_from_ptr<'a>(value: *const c_char, name: &str) -> Result<&'a str, String> {
    if value.is_null() {
        return Err(format!("{name} is null"));
    }

    // SAFETY: the caller promises a valid NUL-terminated C string for the
    // duration of this call.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map_err(|error| format!("{name} is not valid UTF-8: {error}"))
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_harness_create_embedded(workspace: *const c_char) -> *mut Harness {
    clear_error();

    let workspace = match unsafe { string_from_ptr(workspace, "workspace") } {
        Ok(workspace) => workspace,
        Err(error) => {
            set_error(error);
            return ptr::null_mut();
        }
    };

    match Harness::embedded(workspace) {
        Ok(harness) => Box::into_raw(Box::new(harness)),
        Err(error) => {
            set_error(error);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_harness_destroy(handle: *mut Harness) {
    if handle.is_null() {
        return;
    }

    // SAFETY: handles are created by yeet_harness_create_embedded and ownership
    // is transferred back exactly once here.
    unsafe {
        drop(Box::from_raw(handle));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_harness_send_json(handle: *mut Harness, command_json: *const c_char) -> i32 {
    clear_error();

    if handle.is_null() {
        set_error("harness handle is null");
        return -1;
    }

    let command_json = match unsafe { string_from_ptr(command_json, "command_json") } {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };

    let command = match serde_json::from_str::<HarnessCommand>(command_json) {
        Ok(command) => command,
        Err(error) => {
            set_error(format!("decode harness command: {error}"));
            return -1;
        }
    };

    // SAFETY: the handle is exclusively owned by the native host, which must
    // serialize access to it.
    let harness = unsafe { &mut *handle };
    match harness.send(command) {
        Ok(()) => 0,
        Err(error) => {
            set_error(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_harness_try_recv_json(
    handle: *mut Harness,
    output: *mut *mut c_char,
) -> i32 {
    clear_error();

    if handle.is_null() {
        set_error("harness handle is null");
        return -1;
    }
    if output.is_null() {
        set_error("output pointer is null");
        return -1;
    }

    // SAFETY: output is a valid pointer supplied by the caller.
    unsafe {
        *output = ptr::null_mut();
    }

    // SAFETY: the handle is exclusively owned by the native host, which must
    // serialize access to it.
    let harness = unsafe { &mut *handle };
    let Some(event) = harness.try_recv() else {
        return 0;
    };

    match serde_json::to_string(&event) {
        Ok(json) => {
            // SAFETY: output is valid for the duration of this call.
            unsafe {
                *output = string_to_raw(json);
            }
            1
        }
        Err(error) => {
            set_error(format!("encode harness event: {error}"));
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_harness_state_json(handle: *mut Harness) -> *mut c_char {
    clear_error();

    if handle.is_null() {
        set_error("harness handle is null");
        return ptr::null_mut();
    }

    // SAFETY: the handle is exclusively owned by the native host, which must
    // serialize access to it.
    let harness = unsafe { &mut *handle };
    let Some(state) = harness.latest_state() else {
        return ptr::null_mut();
    };

    match serde_json::to_string(state) {
        Ok(json) => string_to_raw(json),
        Err(error) => {
            set_error(format!("encode harness state: {error}"));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_harness_take_last_error() -> *mut c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow_mut()
            .take()
            .map(string_to_raw)
            .unwrap_or(ptr::null_mut())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn yeet_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }

    // SAFETY: this function only accepts pointers returned by string_to_raw.
    unsafe {
        drop(CString::from_raw(value));
    }
}
