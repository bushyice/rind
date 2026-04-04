use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use libc::{geteuid, getgid, getpid, getuid};

use crate::Message;

pub fn send_message(mut msg: Message) -> anyhow::Result<Message> {
  let mut stream = UnixStream::connect("/tmp/rind.sock")?;

  let euid = unsafe { geteuid() };
  let gid = unsafe { getgid() };
  let uid = unsafe { getuid() };
  if euid == 0 {
    msg = msg
      .from_uid(uid)
      .from_gid(gid)
      .from_pid(unsafe { getpid() });
  };

  let payload = serde_json::to_vec(&msg)?;
  let len = (payload.len() as u32).to_be_bytes();

  stream.write_all(&len)?;
  stream.write_all(&payload)?;

  let mut len_buf = [0u8; 4];
  stream.read_exact(&mut len_buf)?;
  let len = u32::from_be_bytes(len_buf) as usize;

  let mut buf = vec![0u8; len];
  stream.read_exact(&mut buf)?;

  Ok(serde_json::from_slice(&buf)?)
}
