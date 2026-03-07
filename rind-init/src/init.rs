use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::FromRawFd;
use std::process::{Child, Command, Stdio};

use libc;
use rind_common::error::{install_panic_handler, report_error, rw_read};
use rind_core::error::rw_write;
use rind_core::store::STORE;
use rind_core::{config, mount, services, units};
use rind_daemon::start_daemon;

fn spawn_tty(tty_path: &str) -> Option<Child> {
  let Ok(tty) = OpenOptions::new().read(true).write(true).open(tty_path) else {
    eprintln!("TTY file {tty_path} not found");
    return None;
  };

  let fd = tty.as_raw_fd();
  let out_fd = unsafe { libc::dup(fd) };
  let err_fd = unsafe { libc::dup(fd) };
  if out_fd < 0 || err_fd < 0 {
    eprintln!("Failed to duplicate tty fd for {tty_path}");
    return None;
  }

  let stdin = unsafe { Stdio::from_raw_fd(fd) };
  let stdout = unsafe { Stdio::from_raw_fd(out_fd) };
  let stderr = unsafe { Stdio::from_raw_fd(err_fd) };

  let shell_exec = rw_read(&config::CONFIG, "config read in spawn_tty")
    .shell
    .exec
    .to_string();
  match Command::new(shell_exec)
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
  install_panic_handler("init");

  // loading untis
  match units::load_units() {
    Err(e) => eprintln!("Error Happened: {e}"),
    Ok(_) => {}
  };

  {
    rw_write(&STORE, "load enabled").load_enabled();
  }

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
    let tty = rw_read(&config::CONFIG, "config read in main tty")
      .shell
      .tty
      .to_string();
    let child = spawn_tty(&tty);

    if let Some(mut child) = child {
      if let Err(err) = child.wait() {
        report_error("Failed to wait for shell", err);
      }
    }
  });

  // keep alive
  loop {
    std::thread::park();
  }
}
