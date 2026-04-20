use rind_core::utils::read_env_file;
use std::fs::OpenOptions;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

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

fn resolve_params() -> (String, String, u64) {
  let raw = std::env::var("RIND_USER_ACTIVE").unwrap_or_default();
  if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
    println!("{v:?}");
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

fn main() {
  let (tty, user, session_id) = resolve_params();
  let Ok(file) = OpenOptions::new().read(true).write(true).open(tty.as_str()) else {
    eprintln!("TTY file {} not found", tty);
    return;
  };

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

  let _ = unsafe { libc::ioctl(fd, libc::TIOCSCTTY, 0) };

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

  let mut cmd = Command::new(&shell);
  cmd.uid(uid);
  cmd.gid(gid);
  cmd.current_dir(&home);

  let mut child = match cmd
    .arg("-i")
    .env_clear()
    .env("HOME", &home)
    .env("USER", user)
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

  let _ = child.wait();
}
