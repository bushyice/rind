use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

fn endpoint() -> &'static str {
  "tr_demo@user_active"
}

fn socket_path() -> String {
  format!("/run/rind-tp/{}.sock", endpoint())
}

fn tty_path() -> String {
  println!("ttypath: {:?}", std::env::var("RIND_LOGIN_TTY"));
  std::env::var("RIND_LOGIN_TTY").unwrap_or_else(|_| "/dev/tty1".to_string())
}

fn tty_name(tty: &str) -> String {
  tty.rsplit('/').next().unwrap_or("tty1").to_string()
}

fn connect_with_retry(path: &str) -> UnixStream {
  loop {
    match UnixStream::connect(path) {
      Ok(stream) => return stream,
      Err(err) => {
        eprintln!("waiting for {path}: {err}");
        thread::sleep(Duration::from_millis(250));
      }
    }
  }
}

fn prompt_login(tty: &str) -> Option<String> {
  let file = OpenOptions::new().read(true).write(true).open(tty).ok()?;
  let mut writer = file.try_clone().ok()?;
  let mut reader = BufReader::new(file);
  let mut line = String::new();

  if write!(writer, "rind login: ").is_err() {
    return None;
  }
  if writer.flush().is_err() {
    return None;
  }
  if reader.read_line(&mut line).ok()? == 0 {
    return None;
  }

  let user = line.trim().to_string();
  if user.is_empty() { None } else { Some(user) }
}

fn send_login_state(user: &str, seat: &str) {
  let path = socket_path();
  let mut stream = connect_with_retry(path.as_str());
  let payload = serde_json::json!({
    "id": user,
    "user": user,
    "seat": seat
  });
  let msg = serde_json::json!({
    "type": "State",
    "name": endpoint(),
    "payload": {
      "Json": payload.to_string()
    },
    "action": "Set"
  });
  let _ = writeln!(stream, "{msg}");
}

fn main() {
  let tty = tty_path();
  let seat = tty_name(tty.as_str());

  let Some(user) = prompt_login(tty.as_str()) else {
    return;
  };
  send_login_state(user.as_str(), seat.as_str());

  loop {
    thread::sleep(Duration::from_secs(5));
  }
}
