use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> std::io::Result<()> {
  let fd = 3;
  let listener = if let Ok(_) = std::env::var("TSOCK_PATH") {
    let path = PathBuf::from("/tmp/some.sock");
    if path.exists() {
      std::fs::remove_file(&path)?;
    }
    UnixListener::bind(path)?
  } else {
    unsafe { UnixListener::from_raw_fd(fd) }
  };
  listener.set_nonblocking(true)?;

  let mut last_activity = Instant::now();
  let idle_timeout = Duration::from_secs(5);

  println!("[ipc_socket] ready");

  loop {
    match listener.accept() {
      Ok((mut stream, _)) => {
        println!("[ipc_socket] accepted connection");
        last_activity = Instant::now();

        handle_connection(&mut stream)?;
      }

      Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
        if last_activity.elapsed() > idle_timeout {
          println!("[ipc_socket] idle, shutting down");
          break;
        }

        std::thread::sleep(Duration::from_millis(100));
      }

      Err(e) => {
        eprintln!("accept error: {}", e);
      }
    }
  }

  println!("[ipc_socket] shutting down");
  Ok(())
}

fn handle_connection(stream: &mut UnixStream) -> std::io::Result<()> {
  let mut buf = [0u8; 1024];

  loop {
    match stream.read(&mut buf) {
      Ok(0) => {
        println!("[ipc_socket] client disconnected");
        break;
      }

      Ok(n) => {
        println!(
          "[ipc_socket] received: {}",
          String::from_utf8_lossy(&buf[..n])
        );

        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")?;
      }

      Err(e) => {
        eprintln!("read error: {}", e);
        break;
      }
    }
  }

  Ok(())
}
