use libloading::{Library, Symbol};
use std::{ffi::CString, os::raw::c_char};

fn main() {
  unsafe {
    let lib = Library::new("target/debug/librind_api.so").unwrap();

    let func: Symbol<unsafe extern "C" fn(u32, *const c_char)> = lib.get(b"init_tp").unwrap();

    func(1, CString::new("ss").unwrap().as_ptr());
  }
}
