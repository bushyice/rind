use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

fn endpoint() -> String {
  std::env::var("RIND_UDS_ENDPOINT").unwrap_or_else(|_| "tp_demo@uds_echo".to_string())
}

fn socket_path() -> String {
  format!("/run/rind-tp/{}.sock", endpoint())
}

fn connect_with_retry(path: &str) -> UnixStream {
  loop {
    match UnixStream::connect(path) {
      Ok(stream) => return stream,
      Err(err) => {
        eprintln!("waiting for {path}: {err}");
        thread::sleep(Duration::from_millis(300));
      }
    }
  }
}

fn main() {
  let path = socket_path();
  let mut stream = connect_with_retry(path.as_str());
  let reader_stream = stream.try_clone().expect("failed to clone uds stream");
  let mut reader = BufReader::new(reader_stream);

  println!("example-uds connected to {path}");
  let mut line = String::new();
  loop {
    line.clear();
    let Ok(read) = reader.read_line(&mut line) else {
      break;
    };
    if read == 0 {
      thread::sleep(Duration::from_millis(100));
      continue;
    }
    let payload = line.trim();
    if payload.is_empty() {
      continue;
    }

    println!("uds_in: {payload}");

    // Emit a demo signal back into rind on each received message.
    let reply = serde_json::json!({
      "type": "Signal",
      "name": "tp_demo@demo_ping",
      "payload": { "String": format!("echo:{payload}") },
      "action": "Set"
    });
    if writeln!(stream, "{reply}").is_err() {
      break;
    }
  }
}
