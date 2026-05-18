use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use libc::{geteuid, getgid, getpid, getuid};
use rind_core::error::{CoreError, CoreResult};

use crate::Message;

pub fn send_message(mut msg: Message) -> CoreResult<Message> {
  let mut stream = UnixStream::connect(
    std::env::var("RIND_SOC_PATH")
      .map(PathBuf::from)
      .unwrap_or_else(|_| PathBuf::from("/tmp/rind.sock")),
  )?;

  let euid = unsafe { geteuid() };
  let gid = unsafe { getgid() };
  let uid = unsafe { getuid() };
  if euid == 0 {
    msg = msg
      .from_uid(uid)
      .from_gid(gid)
      .from_pid(unsafe { getpid() });
  };

  msg
    .write_signed(&mut stream)
    .map_err(|e| CoreError::Custom(e.to_string()))?;

  Ok(Message::read_signed(&mut stream).map_err(|e| CoreError::Custom(e.to_string()))?)
}
