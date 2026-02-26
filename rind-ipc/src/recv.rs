use super::Message;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;

type ClientHandler = fn(Message) -> Result<Option<Message>, anyhow::Error>;

pub fn recv_message(mut stream: UnixStream, handle_client: ClientHandler) {
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

    let msg: Message = match toml::from_str(&raw) {
      Ok(m) => m,
      Err(e) => {
        eprintln!("toml parse error: {e}");
        continue;
      }
    };

    let Ok(Some(response)) = handle_client(msg) else {
      continue;
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

pub fn start_ipc_server(handle_client: ClientHandler) -> std::io::Result<()> {
  let socket_path = "/tmp/rind.sock";
  let _ = std::fs::remove_file(socket_path);
  let listener = UnixListener::bind(socket_path)?;

  println!("Daemon IPC listening on {}", socket_path);

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        thread::spawn(move || recv_message(stream, handle_client));
      }
      Err(e) => eprintln!("IPC connection failed: {}", e),
    }
  }

  Ok(())
}
