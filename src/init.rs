use std::fs::{self, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::process::{Child, Command, Stdio};

use libc;
use nix::mount::{MsFlags, mount};
mod config;
mod daemon;
mod messaging;
mod mount;
mod names;
mod services;
mod units;

fn spawn_tty(tty_path: &str) -> Option<Child> {
  let Ok(tty) = OpenOptions::new().read(true).write(true).open(tty_path) else {
    eprintln!("TTY file {tty_path} not found");
    return None;
  };

  let fd = tty.as_raw_fd();

  let stdin = unsafe { Stdio::from_raw_fd(fd) };
  let stdout = unsafe { Stdio::from_raw_fd(libc::dup(fd)) };
  let stderr = unsafe { Stdio::from_raw_fd(libc::dup(fd)) };

  match Command::new("/bin/sh")
    .stdin(stdin)
    .stdout(stdout)
    .stderr(stderr)
    .spawn()
  {
    Ok(c) => Some(c),
    Err(e) => {
      eprintln!("Failed to start shell: {e}");
      None
    }
  }
}

fn main() {
  fs::create_dir_all("/proc").ok();
  fs::create_dir_all("/sys").ok();
  fs::create_dir_all("/dev").ok();
  fs::create_dir_all("/tmp").ok();

  mount(
    Some("proc"),
    "/proc",
    Some("proc"),
    MsFlags::empty(),
    None::<&str>,
  )
  .ok();
  mount(
    Some("sysfs"),
    "/sys",
    Some("sysfs"),
    MsFlags::empty(),
    None::<&str>,
  )
  .ok();
  mount(
    Some("devtmpfs"),
    "/dev",
    Some("devtmpfs"),
    MsFlags::empty(),
    None::<&str>,
  )
  .ok();
  mount(
    Some("tmpfs"),
    "/tmp",
    Some("tmpfs"),
    MsFlags::empty(),
    None::<&str>,
  )
  .ok();

  // config::CONFIG.lock().unwrap().services.path = ".artifacts/services".into();

  match units::load_units() {
    Err(e) => eprintln!("Error Happened: {e}"),
    Ok(_) => {}
  };

  services::start_services();

  std::thread::spawn(|| {
    let child = spawn_tty("/dev/tty1");

    if let Some(mut child) = child {
      child.wait().expect("Failed to wait for shell");
    }
  });

  std::thread::spawn(|| services::service_loop());

  std::thread::spawn(|| daemon::start_ipc_server());

  loop {
    std::thread::park();
  }
}
