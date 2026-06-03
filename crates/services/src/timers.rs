use std::time::{Duration, Instant};

use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
pub use rind_core::events::ServiceEventKind;
use rind_core::prelude::*;
use rind_primitives::variables::VariableHeap;
use std::os::fd::{AsFd, AsRawFd};

pub use rind_flow::FacetGraph;
use rind_flow::Trigger;
use rind_flow::triggers::trigger_events;

#[model(
  meta_name = name,
  meta_fields(name, duration, after, finish),
  derive_metadata(Debug, Default)
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

#[runtime("timer")]
impl TimerRuntime {
  fn start(name: Ustr) {
    if ctx.registry.as_one::<Timer>("*", name.clone()).is_ok() {
      return Ok(None);
    }

    let timer = ctx
      .registry
      .instantiate_one::<Timer>("*", name.clone(), |metadata| {
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

  fn stop(name: Ustr) {
    let timer = ctx.registry.uninstantiate_one::<Timer>("*", name)?;
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

  fn reconcile_timers(service: Ustr, action: ServiceEventKind) {
    let service_name = normalize_uaddr(service, "");
    let service_name = Ustr::from(service_name.as_str().split('@').next().unwrap_or(""));

    let mut dependents = Vec::new();
    for meta_name in ctx.registry.metadata.metadata_names() {
      let Some(meta) = ctx.registry.metadata.metadata(meta_name.clone()) else {
        continue;
      };
      for group in meta.groups() {
        if let Some(timers) = ctx
          .registry
          .metadata
          .group_items::<Timer>(meta_name.clone(), group.clone())
        {
          for timer in timers {
            if let Some(ref dependencies) = timer.after
              && dependencies.contains(&service_name)
            {
              dependents.push(Ustr::from(format!("{}:{}", group, timer.name)));
            }
          }
        }
      }
    }

    match action {
      ServiceEventKind::Started => {
        for dependent in dependents {
          let _ = dispatch.dispatch("timer", "start", rpayload!({ "name": dependent }));
        }
      }
      ServiceEventKind::Stopped | ServiceEventKind::Failed | ServiceEventKind::Exited { .. } => {
        for dependent in dependents {
          let _ = dispatch.dispatch("timer", "stop", rpayload!({ "name": dependent }));
        }
      }
    }
  }

  fn finish_timer(name: Ustr) {
    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), _>(
        (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
        |registry, (sm, _)| {
          if let Ok(timer) = registry.uninstantiate_one::<Timer>("*", name) {
            if let Some(triggers) = &timer.metadata.finish {
              trigger_events(triggers.clone(), Some(sm), dispatch);
            }
            ctx.resources.terminate(timer.fd);
          }
          Ok(Void)
        },
      )?;
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::parse_duration;

  #[test]
  fn parse_duration_supports_units_and_raw_seconds() {
    assert_eq!(parse_duration("5s"), Some(Duration::from_secs(5)));
    assert_eq!(parse_duration("3m"), Some(Duration::from_secs(180)));
    assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86400)));
    assert_eq!(parse_duration("12"), Some(Duration::from_secs(12)));
  }

  #[test]
  fn parse_duration_rejects_invalid_values() {
    assert_eq!(parse_duration(""), None);
    assert_eq!(parse_duration("foo"), None);
    assert_eq!(parse_duration("xs"), None);
  }

  #[test]
  fn parse_duration_property_seconds_equivalence() {
    for n in 0u64..500 {
      let raw = parse_duration(&n.to_string());
      let with_suffix = parse_duration(&format!("{n}s"));
      assert_eq!(raw, Some(Duration::from_secs(n)));
      assert_eq!(with_suffix, raw);
    }
  }
}
