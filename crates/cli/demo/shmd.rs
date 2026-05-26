use std::error::Error;
use std::thread;

use libloading::os::unix::{Library, Symbol};
#[repr(C)]
pub struct rind_tp {
  pub protocol: u8,
  pub options: *const *const std::os::raw::c_char,
  pub len: usize,
  pub id: u64,
}

#[repr(C)]
pub struct rind_msg {
  r#type: u8,
  action: u8,
  payload: *mut rind_payload,
  name: *const std::os::raw::c_char,
}

#[repr(C)]
pub struct rind_payload {
  r#type: u8,
  content: *const std::os::raw::c_char,
}

unsafe extern "C" fn print_output(msg: rind_msg) {
  println!("Received message: {:?}", msg.name);
}

fn main() -> Result<(), Box<dyn Error>> {
  println!("connesting to shm tp...");

  let lib = unsafe { Library::new("/lib/librind_api.so")? };

  let rind_listen_tp: Symbol<
    unsafe extern "C" fn(tp: *mut rind_tp, func: unsafe extern "C" fn(rind_msg)),
  > = unsafe { lib.get(b"rind_listen_tp").unwrap() };

  let rind_log_msg: Symbol<unsafe extern "C" fn(log: *const std::os::raw::c_char) -> rind_msg> =
    unsafe { lib.get(b"rind_log_msg").unwrap() };

  let rind_send_message: Symbol<unsafe extern "C" fn(tp: *const rind_tp, message: rind_msg) -> u8> =
    unsafe { lib.get(b"rind_send_message").unwrap() };

  let rind_init_tp: Symbol<
    unsafe extern "C" fn(protocol: u8, options: *const std::os::raw::c_char) -> rind_tp,
  > = unsafe { lib.get(b"rind_init_tp").unwrap() };

  let path = std::ffi::c_str::CString::new(
    std::env::var("RIND_TP_SOCK").unwrap_or("/run/rind-tp/shm.sock".to_string()),
  )
  .unwrap();
  let tp = Box::into_raw(Box::new(unsafe { rind_init_tp(2, path.as_ptr()) }));
  println!("shm tp connected and mapped");

  unsafe {
    rind_listen_tp(tp, print_output);
  }

  println!("sending message to rind...");
  let msg = unsafe {
    rind_log_msg(
      std::ffi::c_str::CString::new("hello from shm client!")
        .unwrap()
        .as_ptr(),
    )
  };
  unsafe {
    rind_send_message(tp, msg);
  }

  loop {
    thread::sleep(std::time::Duration::from_secs(1));
  }
}
