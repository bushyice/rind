use std::{fs::OpenOptions, io::Write, os::fd::AsRawFd};

use rind_core::prelude::*;
use rind_core::reexports::libc;
use rind_plugins::prelude::*;
use rind_plugins_common::TTYPayload;

plugin_extensible!(EXTENSIONS);

#[derive(Default)]
struct DisplayOrchestrator;

impl Orchestrator for DisplayOrchestrator {
  fn id(&self) -> &str {
    "display"
  }

  fn depends_on(&self) -> &[&str] {
    &["ttys"]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("display", "bootstrap", Default::default())?;
    Ok(())
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![Box::new(DisplayRuntime::default())]
  }
}

#[derive(Default)]
pub struct DisplayRuntime;

impl Runtime for DisplayRuntime {
  fn id(&self) -> &str {
    "display"
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    _ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "bootstrap" => {
        if let Ok(mut file) = OpenOptions::new().write(true).open("/dev/tty2") {
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
        }
      }
      _ => {}
    }

    Ok(None)
  }
}

fn take_tty(name: &str, taken: TTYPayload) -> CoreResult<TTYPayload> {
  match name {
    "boot" => {
      if taken.taken().contains(&"tty2".to_ustr()) {
        return Ok(TTYPayload::Check);
      }
      Ok(TTYPayload::Take("tty2".to_ustr()))
    }
    _ => Ok(TTYPayload::Check),
  }
}

plugin!(
  name: "display",
  version: 1,
  caps: PluginCapability::ORCHESTRATORS | PluginCapability::RUNTIMES | PluginCapability::EXTENSIBLE,
  deps: &["ttys"],
  create: DisplayPlugin,
  orchestrators: [DisplayOrchestrator::default()],
  extensions: [resolve(take_tty)],
  struct DisplayPlugin;
);

plugin_abi!(1);
