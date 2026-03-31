/*
 * TODO: Userspace Update
 * - stuff
 */

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::thread;
use std::time::Duration;

use rind_ipc::{LoginPayload, Message, MessageType, send::send_message};

fn tty_path() -> String {
  let path = std::env::var("RIND_LOGIN_TTY")
    .map(|x| {
      if x.is_empty() {
        "/dev/tty1".to_string()
      } else {
        x
      }
    })
    .unwrap_or_else(|_| "/dev/tty1".to_string());
  println!("login on tty: {:?}", path);
  path
}

fn prompt_login(
  writer: &mut File,
  reader: &mut BufReader<File>,
) -> Option<(String, Option<String>)> {
  let mut user_line = String::new();
  let mut pass_line = String::new();

  let _ = write!(writer, "\x1b[2J\x1b[H");
  let _ = writer.flush();

  thread::sleep(Duration::from_secs(1));

  if write!(writer, "rind login: ").is_err() || writer.flush().is_err() {
    return None;
  }
  if reader.read_line(&mut user_line).ok()? == 0 {
    return None;
  }

  if write!(writer, "password: ").is_err() || writer.flush().is_err() {
    return None;
  }
  if reader.read_line(&mut pass_line).ok()? == 0 {
    return None;
  }

  let user = user_line.trim().to_string();
  let pass = pass_line.trim().to_string();

  if user.is_empty() {
    None
  } else {
    Some((user, if pass.is_empty() { None } else { Some(pass) }))
  }
}

fn send_login_state(user: &str, pass: Option<String>, tty: &str, writer: &mut File) -> bool {
  let payload = LoginPayload {
    username: user.to_string(),
    password: pass,
    tty: tty.to_string(),
  };

  let msg = Message::from_type(MessageType::Login).with(serde_json::to_string(&payload).unwrap());

  match send_message(msg) {
    Err(e) => {
      let _ = write!(writer, "{e}");
      let _ = writer.flush();
      false
    }
    Ok(_) => {
      let _ = write!(writer, "\x1b[2J\x1b[H");
      let _ = writer.flush();
      true
    }
  }
}

pub fn prompt_and_login(writer: &mut File, reader: &mut BufReader<File>, tty: String) {
  let Some((user, pass)) = prompt_login(writer, reader) else {
    return prompt_and_login(writer, reader, tty.clone());
  };

  if !send_login_state(user.as_str(), pass, tty.as_str(), writer) {
    prompt_and_login(writer, reader, tty.clone());
  }
}

fn main() {
  let tty = tty_path();

  let file = OpenOptions::new()
    .read(true)
    .write(true)
    .open(tty.clone())
    .expect("Failed to open tty");
  let mut writer = file.try_clone().ok().expect("Failed to open writer");
  let mut reader = BufReader::new(file);

  prompt_and_login(&mut writer, &mut reader, tty.clone());

  loop {
    thread::sleep(Duration::from_secs(5));
  }
}
