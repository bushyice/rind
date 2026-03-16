use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use crate::Message;

pub fn send_message(msg: Message) -> anyhow::Result<Message> {
  let mut stream = UnixStream::connect("/tmp/rind.sock")?;

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
