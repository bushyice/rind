use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use libc::seteuid;
use rind_core::utils::read_env_file;
use rind_ipc::{
  Message, MessageType,
  payloads::{LoginPayload, LogoutPayload},
  send::send_message,
  ser::ser_to_vec,
};

use crate::{handle_message, handle_send, handle_send_raw, send_msg};

#[derive(Parser)]
#[command(name = "syssess")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"), "-", env!("BUILD_HASH")))]
pub struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  Logout,
  #[cfg(feature = "syssess-login")]
  Login,
  #[cfg(feature = "syssess-usershell")]
  Shell,
}

pub fn main() {
  let cli = Cli::parse();

  match cli.command {
    Command::Logout => {
      let username = std::env::var("USER").expect("unknown user");
      let session_id = std::env::var("SESSION_ID")
        .expect("unknown session")
        .parse::<u64>()
        .expect("unknown session");
      handle_send!(
        "logout",
        &LogoutPayload {
          session_id,
          username,
          seat: None
        }
      );
    }
    #[cfg(feature = "syssess-login")]
    Command::Login => {
      let tty = login_tty_path();
      let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(tty.clone())
        .expect("Failed to open tty");
      let mut writer = file.try_clone().ok().expect("Failed to open writer");
      let mut reader = BufReader::new(file);
      prompt_and_login(&mut writer, &mut reader, tty);
      std::process::exit(0);
    }
    #[cfg(feature = "syssess-usershell")]
    Command::Shell => {
      let (tty, user, session_id) = resolve_shell_params();
      println!("{user} session in {tty} as {session_id}");
      let Ok(file) = OpenOptions::new().read(true).write(true).open(tty.as_str()) else {
        eprintln!("TTY file {} not found", tty);
        return;
      };

      use std::os::unix::process::CommandExt;

      let fd = file.as_raw_fd();
      let out_fd = unsafe { libc::dup(fd) };
      let err_fd = unsafe { libc::dup(fd) };
      if out_fd < 0 || err_fd < 0 {
        eprintln!("failed to duplicate tty fd for {}", tty);
        return;
      }

      let stdin = unsafe { Stdio::from_raw_fd(fd) };
      let stdout = unsafe { Stdio::from_raw_fd(out_fd) };
      let stderr = unsafe { Stdio::from_raw_fd(err_fd) };

      std::thread::sleep(std::time::Duration::from_millis(100));

      for i in 0..5 {
        if unsafe { libc::ioctl(fd, libc::TIOCSCTTY, 1) } == 0 {
          let mut tty_attrs: libc::termios = unsafe { std::mem::zeroed() };
          if unsafe { libc::tcgetattr(fd, &mut tty_attrs) } == 0 {
            tty_attrs.c_iflag = libc::ICRNL | libc::IXON | libc::IXOFF;
            tty_attrs.c_oflag = libc::OPOST | libc::ONLCR;
            tty_attrs.c_cflag = libc::CREAD | libc::CS8 | libc::HUPCL;
            tty_attrs.c_lflag =
              libc::ISIG | libc::ICANON | libc::ECHO | libc::ECHOE | libc::ECHOK | libc::IEXTEN;

            tty_attrs.c_cc[libc::VERASE] = 0x7f;

            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &tty_attrs) };
          }
          break;
        }
        let err = std::io::Error::last_os_error();
        eprintln!("attempt {} to acquire tty failed: {}", i + 1, err);
        std::thread::sleep(std::time::Duration::from_millis(200));
      }

      let mut shell = "/bin/sh".to_string();
      let mut uid = 0u32;
      let mut gid = 0u32;
      let mut home = "/root".to_string();

      if let Some((user_uid, user_gid, user_home, user_shell)) = get_user_info(&user) {
        uid = user_uid;
        gid = user_gid;
        home = user_home;
        shell = user_shell;
      }

      let mut extra_env = if uid == 0 && user == "root" {
        read_env_file("/root/.env")
      } else {
        read_env_file(&format!("{home}/.env"))
      };

      let mut cmd = std::process::Command::new(&shell);
      cmd.uid(uid);
      cmd.gid(gid);
      cmd.current_dir(&home);

      let mut child = match cmd
        .arg("-i")
        .env_clear()
        .env("HOME", &home)
        .env("USER", &user)
        .env("TERM", "linux")
        .env("SESSION_ID", session_id.to_string())
        .env(
          "PATH",
          extra_env
            .remove("PATH")
            .unwrap_or_else(|| "/usr/bin:/bin".to_string()),
        )
        .envs(extra_env.drain())
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
      {
        Ok(child) => child,
        Err(err) => {
          eprintln!("failed to start shell '{}': {err}", shell);
          return;
        }
      };

      unsafe {
        cmd.pre_exec(move || {
          libc::setsid();
          if libc::ioctl(fd, libc::TIOCSCTTY, 1) < 0 {
            return Err(std::io::Error::last_os_error());
          }
          let pid = libc::getpid();
          libc::tcsetpgrp(fd, pid);
          Ok(())
        });
      }

      let status = child.wait().unwrap();
      if status.success() {
        unsafe {
          // just so rind thinks this is
          // an actual user from this session
          seteuid(uid);
        }
        handle_send!(
          "logout",
          &LogoutPayload {
            session_id,
            username: user,
            seat: None
          }
        );
      }
    }
  }
}

fn login_tty_path() -> String {
  let path = std::env::var("RIND_LOGIN_TTY")
    .map(|x| {
      if x.is_empty() {
        "/dev/tty1".to_string()
      } else if x.starts_with("seat") {
        format!(
          "/dev/tty{}",
          x.trim_start_matches("seat").parse::<u32>().unwrap_or(0) + 1
        )
      } else {
        format!("/dev/{x}")
      }
    })
    .unwrap_or_else(|_| "/dev/tty1".to_string());
  path
}

fn get_hostname() -> String {
  std::env::var("RIND_LOGIN_HOSTNAME").unwrap_or_else(|_| {
    let mut buf = [0u8; 256];
    unsafe {
      libc::gethostname(buf.as_mut_ptr().cast(), buf.len());
    }
    let s = String::from_utf8_lossy(&buf).to_string();
    s.trim_matches('\0').to_string()
  })
}

fn prompt_login(
  writer: &mut File,
  reader: &mut BufReader<File>,
) -> Option<(String, Option<String>)> {
  let mut user_line = String::new();
  let _ = write!(writer, "\x1b[2J\x1b[H");
  if let Ok(issue) = std::fs::read_to_string("/etc/issue") {
    let _ = write!(writer, "{issue}");
  }
  let _ = writer.flush();
  // thread::sleep(Duration::from_secs(1));
  if write!(writer, "{} login: ", get_hostname()).is_err() || writer.flush().is_err() {
    return None;
  }
  if reader.read_line(&mut user_line).ok()? == 0 {
    return None;
  }
  if write!(writer, "password: ").is_err() || writer.flush().is_err() {
    return None;
  }
  let password = rpassword::read_password().unwrap();
  let user = user_line.trim().to_string();
  let pass = password.trim().to_string();
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
    seat: tty.to_string(),
  };
  let msg = Message::from("login").with(ser_to_vec(&payload, false));
  match send_message(msg) {
    Err(e) => {
      let _ = write!(writer, "\x1b[2J\x1b[HError: {e}\n");
      let _ = writer.flush();
      thread::sleep(Duration::from_secs(2));
      false
    }
    Ok(response) => {
      if let MessageType::Error = response.r#type {
        let err_msg = response
          .payload
          .as_ref()
          .map(|p| rind_ipc::ser::deser_string(p))
          .unwrap_or_else(|| "login failed".to_string());
        let _ = write!(writer, "\x1b[2J\x1b[HError: {err_msg}\n");
        let _ = writer.flush();
        thread::sleep(Duration::from_secs(2));
        false
      } else {
        let _ = write!(writer, "\x1b[2J\x1b[H");
        let _ = writer.flush();
        true
      }
    }
  }
}

fn prompt_and_login(writer: &mut File, reader: &mut BufReader<File>, tty: String) {
  let Some((user, pass)) = prompt_login(writer, reader) else {
    return prompt_and_login(writer, reader, tty.clone());
  };
  if !send_login_state(user.as_str(), pass, tty.as_str(), writer) {
    prompt_and_login(writer, reader, tty.clone());
  }
}

fn get_user_info(username: &str) -> Option<(u32, u32, String, String)> {
  let file = std::fs::read_to_string("/etc/passwd").ok()?;
  for line in file.lines() {
    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() >= 7 && parts[0] == username {
      let uid = parts[2].parse().ok()?;
      let gid = parts[3].parse().ok()?;
      let home = parts[5].to_string();
      let shell = parts[6].to_string();
      return Some((uid, gid, home, shell));
    }
  }
  None
}

fn resolve_shell_params() -> (String, String, u64) {
  let raw = std::env::var("RIND_USER_ACTIVE").unwrap_or_default();
  if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
    return (
      if let Some(tty) = v.get("tty").and_then(|x| x.as_str()) {
        if tty.starts_with("/dev/") {
          tty.to_string()
        } else {
          format!("/dev/{tty}")
        }
      } else if let Some(seat) = v.get("seat").and_then(|x| x.as_str()) {
        if seat.starts_with("/dev/") {
          seat.to_string()
        } else {
          format!("/dev/{seat}")
        }
      } else {
        "/dev/tty1".to_string()
      },
      if let Some(username) = v.get("username").and_then(|x| x.as_str()) {
        username.to_string()
      } else {
        "unknown".into()
      },
      if let Some(id) = v.get("session_id").and_then(|x| x.as_u64()) {
        id
      } else {
        1u64
      },
    );
  }
  (
    std::env::var("RIND_LOGIN_TTY").unwrap_or_else(|_| "/dev/tty1".to_string()),
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()),
    1u64,
  )
}
