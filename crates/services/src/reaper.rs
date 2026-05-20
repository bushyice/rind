use std::collections::HashMap;

use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};

use rind_core::prelude::*;

pub const REAPER_RUNTIME_ID: &str = "reaper";

#[derive(Default)]
pub struct ReaperRuntime;

impl Runtime for ReaperRuntime {
  fn id(&self) -> &str {
    REAPER_RUNTIME_ID
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    _ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "reap_once" => loop {
        match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
          Ok(WaitStatus::Exited(pid, code)) => {
            let mut fields = HashMap::new();
            fields.insert("pid".to_string(), pid.as_raw().to_string());
            fields.insert("code".to_string(), code.to_string());
            log.log(LogLevel::Info, "reaper", "child exited", fields);

            dispatch.dispatch(
              "services",
              "child_exited",
              RuntimePayload::default()
                .insert("pid", pid.as_raw())
                .insert("code", code),
            )?;
          }
          Ok(WaitStatus::Signaled(pid, signal, _)) => {
            let code = 128 + signal as i32;
            let mut fields = HashMap::new();
            fields.insert("pid".to_string(), pid.as_raw().to_string());
            fields.insert("signal".to_string(), format!("{signal:?}"));
            log.log(LogLevel::Info, "reaper", "child signaled", fields);

            dispatch.dispatch(
              "services",
              "child_exited",
              RuntimePayload::default()
                .insert("pid", pid.as_raw())
                .insert("code", code),
            )?;
          }
          Ok(WaitStatus::StillAlive) | Err(nix::errno::Errno::ECHILD) => break,
          Ok(_) => {}
          Err(e) => {
            let mut fields = HashMap::new();
            fields.insert("error".to_string(), e.to_string());
            log.log(LogLevel::Error, "reaper", "waitpid error", fields);
            break;
          }
        }
      },
      "timeout_sweep" => {
        dispatch.dispatch("services", "timeout_sweep", Default::default())?;
      }
      _ => {}
    }
    Ok(None)
  }
}
