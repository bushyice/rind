use crate::services::SERVICES;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;

fn handle_client(mut stream: UnixStream) {
  let mut buf = [0u8; 1024];
  while let Ok(n) = stream.read(&mut buf) {
    if n == 0 {
      break;
    }
    let cmd = String::from_utf8_lossy(&buf[..n]);
    println!("Command: {cmd}");
    let response = match cmd.trim() {
      "list" => format!(
        "{}\n",
        SERVICES
          .read()
          .unwrap()
          .keys()
          .cloned()
          .collect::<Vec<String>>()
          .join(", ")
      ),
      _ => "unknown command\n".to_string(),
    };
    println!("Writing to output");
    match stream.write_all(response.as_bytes()) {
      Ok(_) => println!("Written to output"),
      Err(e) => eprintln!("{e}"),
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
