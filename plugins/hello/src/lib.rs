//! Example Qell plugin. Compile to `wasm32-unknown-unknown`.

use std::alloc::{alloc, Layout};

#[link(wasm_import_module = "qell")]
extern "C" {
    fn register_command(ptr: *const u8, len: usize);
    fn log(ptr: *const u8, len: usize);
    fn notify(ptr: *const u8, len: usize);
    fn insert_text(ptr: *const u8, len: usize);
    fn selection(ptr: *mut u8, cap: usize) -> usize;
}

fn host_str(f: unsafe extern "C" fn(*const u8, usize), s: &str) {
    unsafe { f(s.as_ptr(), s.len()) }
}

fn current_selection() -> String {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { selection(buf.as_mut_ptr(), buf.len()) };
    if n > buf.len() {
        buf = vec![0u8; n];
        unsafe { selection(buf.as_mut_ptr(), buf.len()) };
    }
    buf.truncate(n);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Host allocates strings it passes to us through this.
#[no_mangle]
pub extern "C" fn qell_alloc(len: usize) -> *mut u8 {
    unsafe { alloc(Layout::from_size_align_unchecked(len.max(1), 1)) }
}

#[no_mangle]
pub extern "C" fn qell_init() {
    host_str(register_command, "hello");
    host_str(register_command, "shout");
    host_str(log, "hello plugin loaded");
}

#[no_mangle]
pub extern "C" fn qell_command(name_ptr: *const u8, name_len: usize, arg_ptr: *const u8, arg_len: usize) -> i32 {
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name_ptr, name_len)) };
    let arg = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(arg_ptr, arg_len)) };
    match name {
        "hello" => {
            let msg = if arg.is_empty() { "Hello from a WASM plugin!".to_string() } else { format!("Hello, {arg}!") };
            host_str(notify, &msg);
            0
        }
        "shout" => {
            let text = current_selection().to_uppercase();
            host_str(insert_text, &text);
            0
        }
        _ => 1,
    }
}
