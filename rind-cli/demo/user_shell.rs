use std::fs::OpenOptions;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::process::{Command, Stdio};

fn resolve_params() -> (String, String) {
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
      if let Some(username) = v.get("id").and_then(|x| x.as_str()) {
        username.to_string()
      } else {
        "unknown".into()
      },
    );
  }

  (
    std::env::var("RIND_LOGIN_TTY").unwrap_or_else(|_| "/dev/tty1".to_string()),
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()),
  )
}

fn main() {
  let (tty, user) = resolve_params();
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

  let mut child = match Command::new("/bin/sh")
    .arg("-i")
    .env("USER", user)
    .stdin(stdin)
    .stdout(stdout)
    .stderr(stderr)
    .spawn()
  {
    Ok(child) => child,
    Err(err) => {
      eprintln!("failed to start /bin/sh: {err}");
      return;
    }
  };

  let _ = child.wait();
}
