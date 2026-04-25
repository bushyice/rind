use std::time::Duration;

use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use rind_core::prelude::*;
use std::os::fd::{AsFd, AsRawFd};

#[derive(Default)]
pub struct TimerRuntime;

impl Runtime for TimerRuntime {
  fn id(&self) -> &str {
    "timer"
  }

  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "start_timer" => {
        let name = payload.get::<Ustr>("name")?;
        let index = payload.get::<usize>("index").ok();
        let duration = payload.get::<Duration>("duration")?;

        let tfd = TimerFd::new(
          ClockId::CLOCK_MONOTONIC,
          TimerFlags::TFD_NONBLOCK | TimerFlags::TFD_CLOEXEC,
        )
        .map_err(CoreError::custom)?;

        tfd
          .set(
            Expiration::OneShot(TimeSpec::from(duration)),
            TimerSetTimeFlags::empty(),
          )
          .map_err(CoreError::custom)?;

        let fd = tfd.as_fd().as_raw_fd();

        ctx.resources.own(fd, tfd);

        let action: ResourceAction = ("timer", "finish_timer").into();

        ctx.resources.action(
          fd,
          action
            .payload(move |payload| payload.insert("name", name.clone()).insert("index", index)),
        );
      }
      "finish_timer" => {
        let fd = payload.get::<i32>("fd")?;
        let name = payload.get::<Ustr>("name")?;
        let index = payload.get::<Option<usize>>("index").unwrap_or(None);

        ctx.resources.terminate(fd);
        let _ = dispatch.dispatch(
          "services",
          "stop",
          rpayload!({
            "name": name,
            "index": index
          }),
        );
      }
      _ => {}
    }

    Ok(None)
  }
}
