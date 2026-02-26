use std::fs::{self, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::process::{Child, Command, Stdio};

use libc;
use rind_core::{mount, services, units};
use rind_daemon::start_daemon;

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
  // loading untis
  match units::load_units() {
    Err(e) => eprintln!("Error Happened: {e}"),
    Ok(_) => {}
  };

  // mount shit
  mount::mount_units();

  // start services
  services::start_services();

  // service waiter
  std::thread::spawn(|| services::service_loop());

  // daemon for cli
  std::thread::spawn(|| match start_daemon() {
    Err(e) => eprintln!("Failed to start daemon: {e}"),
    _ => {}
  });

  // will be removed
  std::thread::spawn(|| {
    let child = spawn_tty("/dev/tty1");

    if let Some(mut child) = child {
      child.wait().expect("Failed to wait for shell");
    }
  });

  // keep alive
  loop {
    std::thread::park();
  }
}
