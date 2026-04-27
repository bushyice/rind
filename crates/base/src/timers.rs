use std::time::{Duration, Instant};

pub use crate::core::events::ServiceEventKind;
pub use crate::prelude::StateMachine;
use crate::prelude::VariableHeap;
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use rind_core::prelude::*;
use std::os::fd::{AsFd, AsRawFd};

use crate::flow::Trigger;
use crate::triggers::trigger_events;

#[model(
  meta_name = name,
  meta_fields(name, duration, after, finish),
  derive_metadata(Debug)
)]
pub struct Timer {
  pub name: Ustr,
  pub duration: Ustr,
  pub after: Option<Vec<Ustr>>,
  pub finish: Option<Vec<Trigger>>,

  pub deadline: Instant,
  pub fd: i32,
}

pub fn parse_duration(s: &str) -> Option<Duration> {
  let s = s.trim();
  if s.is_empty() {
    return None;
  }

  let (num_str, unit) = s.split_at(s.len() - 1);
  if let Ok(num) = num_str.parse::<u64>() {
    match unit {
      "s" => Some(Duration::from_secs(num)),
      "m" => Some(Duration::from_secs(num * 60)),
      "h" => Some(Duration::from_secs(num * 3600)),
      "d" => Some(Duration::from_secs(num * 86400)),
      _ => s.parse().ok().map(Duration::from_secs),
    }
  } else {
    s.parse().ok().map(Duration::from_secs)
  }
}

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
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "start" => {
        let name = payload.get::<Ustr>("name")?;
        let timer_key = Ustr::from(format!("units@{}", name));

        if ctx.registry.instances.contains_key(&timer_key) {
          return Ok(None);
        }

        let timer = ctx
          .registry
          .instantiate_one::<Timer>("units", name.clone(), |metadata| {
            let duration_str = metadata.duration.as_str();
            let duration = parse_duration(duration_str)
              .ok_or_else(|| CoreError::Custom(format!("invalid duration: {}", duration_str)))?;

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

            Ok(Timer {
              metadata,
              deadline: Instant::now() + duration,
              fd,
            })
          })?;

        let res_action: ResourceAction = ("timer", "finish_timer").into();
        ctx.resources.action(
          timer.fd,
          res_action.payload(move |p| p.insert("name", name.clone())),
        );

        log.log(
          LogLevel::Info,
          "timer",
          "started timer",
          [
            ("timer".to_string(), timer.metadata.name.to_string()),
            ("duration".to_string(), timer.metadata.duration.to_string()),
          ]
          .into(),
        );
      }
      "stop" => {
        let name = payload.get::<Ustr>("name")?;

        let timer = ctx.registry.uninstantiate_one::<Timer>("units", name)?;
        ctx.resources.terminate(timer.fd);
        log.log(
          LogLevel::Info,
          "timer",
          "stopped timer",
          [
            ("timer".to_string(), timer.metadata.name.to_string()),
            ("duration".to_string(), timer.metadata.duration.to_string()),
          ]
          .into(),
        );
      }
      "reconcile_timers" => {
        let service_name = normalize_uaddr(payload.get::<Ustr>("service")?, "units@");
        let event_action = payload.get::<ServiceEventKind>("action")?;

        let metadata = ctx
          .registry
          .metadata
          .metadata("units")
          .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;
        let mut dependents = Vec::new();
        for group in metadata.groups() {
          if let Some(timers) = ctx
            .registry
            .metadata
            .group_items::<Timer>("units", group.clone())
          {
            for timer in timers {
              if let Some(ref dependencies) = timer.after
                && dependencies.contains(&service_name)
              {
                dependents.push(Ustr::from(format!("{}@{}", group, timer.name)));
              }
            }
          }
        }

        match event_action {
          ServiceEventKind::Started => {
            for dependent in dependents {
              let _ = dispatch.dispatch("timer", "start", rpayload!({ "name": dependent }));
            }
          }
          ServiceEventKind::Stopped
          | ServiceEventKind::Failed
          | ServiceEventKind::Exited { .. } => {
            for dependent in dependents {
              let _ = dispatch.dispatch("timer", "stop", rpayload!({ "name": dependent }));
            }
          }
        }
      }
      "finish_timer" => {
        let name = payload.get::<Ustr>("name")?;

        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, _)| {
              if let Ok(timer) = registry.uninstantiate_one::<Timer>("units", name) {
                if let Some(triggers) = &timer.metadata.finish {
                  trigger_events(triggers.clone(), Some(sm), dispatch);
                }
                ctx.resources.terminate(timer.fd);
              }
              Ok(())
            },
          )?;
      }
      _ => {}
    }

    Ok(None)
  }
}
