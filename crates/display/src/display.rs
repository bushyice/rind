use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::{fs::OpenOptions, io::Write, os::fd::AsRawFd};

use rind_core::prelude::*;
use rind_core::reexports::{libc, serde_json};
use rind_plugins::prelude::*;
use rind_plugins_common::SeatPayload;

plugin_extensible!(EXTENSIONS);

#[derive(Default)]
struct DisplayOrchestrator;

impl Orchestrator for DisplayOrchestrator {
  fn id(&self) -> &str {
    "display"
  }

  fn depends_on(&self) -> &[&str] {
    &["seatd"]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    DisplayRuntime::actions.bootstrap().orchestrate(ctx)?;
    Ok(Void)
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![Box::new(DisplayRuntime::default())]
  }
}

pub struct DisplayRuntime {
  seat: u32,
}

impl Default for DisplayRuntime {
  fn default() -> Self {
    Self { seat: 2 }
  }
}

fn display_thread(mut file: File) {
  let mut buf = [0u8; 1024];
  let fd = file.as_raw_fd();

  let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
  if flags >= 0 {
    let new_flags = flags & !libc::O_NONBLOCK;
    unsafe {
      libc::fcntl(fd, libc::F_SETFL, new_flags);
    }
  }

  unsafe {
    let mut tty_settings: libc::termios = std::mem::zeroed();
    if libc::tcgetattr(fd, &mut tty_settings) == 0 {
      tty_settings.c_lflag &= !(libc::ICANON | libc::ECHO);
      libc::tcsetattr(fd, libc::TCSANOW, &tty_settings);
    }
  }

  let _ = file.write_all(b"type whitespace to switch back\n");

  loop {
    match file.read(&mut buf) {
      Ok(0) => break,
      Ok(n) => {
        let read_bytes = &buf[..n];

        if read_bytes.contains(&b' ') || read_bytes.contains(&b'\n') {
          let _ = std::process::Command::new("sysinvoke")
            .arg("seat")
            .arg(serde_json::to_string(&SeatPayload::Activate("seat0".to_ustr())).unwrap())
            .spawn()
            .unwrap()
            .wait();
        }
      }
      Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
      Err(e) => {
        eprintln!("Error reading file: {:?}", e);
        break;
      }
    }
  }
}

#[runtime("display")]
impl DisplayRuntime {
  fn bootstrap(&mut self) {
    if let Ok(mut file) = OpenOptions::new()
      .read(true)
      .write(true)
      .open(tty_from_u32(self.seat))
    {
      if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSCTTY, 1) } != 0 {
        // log.log(
        //   LogLevel::Error,
        //   "display",
        //   &format!("Failed to take tty {}", std::io::Error::last_os_error()),
        //   Default::default(),
        // );
      }
      let _ = write!(
        file,
        "\x1b[2J\x1b[H\x1b[32mHello World from Display Plugin!\x1b[0m\n"
      );
      let _ = file.flush();

      std::thread::spawn(move || display_thread(file));
    }
  }
}

fn tty_from_u32(u32: u32) -> PathBuf {
  PathBuf::from(format!("/dev/tty{}", u32))
}

fn seat_from_u32(u32: u32) -> String {
  format!("seat{}", u32 - 1)
}

fn take_seat(name: &str, taken: SeatPayload) -> CoreResult<SeatPayload> {
  match name {
    "boot" => {
      if taken.taken().contains(&seat_from_u32(2).to_ustr()) {
        return Ok(SeatPayload::Check);
      }
      Ok(SeatPayload::Take(seat_from_u32(2).to_ustr()))
    }
    _ => Ok(SeatPayload::Check),
  }
}

plugin!(
  name: "display",
  version: 1,
  caps: PluginCapability::ORCHESTRATORS | PluginCapability::RUNTIMES | PluginCapability::EXTENSIONS,
  deps: &["seatd"],
  create: DisplayPlugin,
  orchestrators: [DisplayOrchestrator::default()],
  extensions: [resolve(take_seat)],
  struct DisplayPlugin;
);

plugin_abi!(1);
