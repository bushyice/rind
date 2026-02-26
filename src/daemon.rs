use crate::messaging::*;
use crate::units::UNITS;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;

fn handle_client(mut stream: UnixStream) {
  println!("client connected");

  loop {
    let mut len_buf = [0u8; 4];
    if let Err(e) = stream.read_exact(&mut len_buf) {
      eprintln!("client disconnected / len read error: {e}");
      break;
    }

    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    if let Err(e) = stream.read_exact(&mut buf) {
      eprintln!("payload read error: {e}");
      break;
    }

    let raw = match String::from_utf8(buf) {
      Ok(s) => s,
      Err(e) => {
        eprintln!("utf8 error: {e}");
        continue;
      }
    };

    println!("raw: {raw}");

    let msg: Message = match toml::from_str(&raw) {
      Ok(m) => m,
      Err(e) => {
        eprintln!("toml parse error: {e}");
        continue;
      }
    };

    println!("parsed: {msg:?}");

    let response = match msg.r#type {
      MessageType::List => Message::from_type(MessageType::List).with_vec(
        UNITS
          .read()
          .unwrap()
          .names()
          .iter()
          .map(|x| x.to_string())
          .collect(),
      ),
      _ => MessageType::Unknown.into(),
    };

    let resp = response.as_string().into_bytes();
    let len = (resp.len() as u32).to_be_bytes();

    if let Err(e) = stream.write_all(&len) {
      eprintln!("write len error: {e}");
      break;
    }

    if let Err(e) = stream.write_all(&resp) {
      eprintln!("write payload error: {e}");
      break;
    }
  }
}

pub fn start_ipc_server() -> std::io::Result<()> {
  let socket_path = "/tmp/rind.sock";
  let _ = std::fs::remove_file(socket_path); // remove if exists
  let listener = UnixListener::bind(socket_path)?;

  println!("Daemon IPC listening on {}", socket_path);

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        thread::spawn(|| handle_client(stream));
      }
      Err(e) => eprintln!("IPC connection failed: {}", e),
    }
  }

  Ok(())
}
