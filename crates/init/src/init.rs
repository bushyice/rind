use std::path::PathBuf;
use std::time::Duration;

use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags};
use nix::sys::signal::{Signal, kill};
use nix::sys::signalfd::{SfdFlags, SigSet, SignalFd};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use nix::unistd::Pid;
use rind_base::flow::FlowRuntime;
use rind_base::ipc::IpcRuntime;
use rind_base::mount::MountRuntime;
use rind_base::networking::NetworkingRuntime;
use rind_base::reaper::ReaperRuntime;
use rind_base::services::ServiceRuntime;
use rind_base::sockets::SocketRuntime;
use rind_base::timers::TimerRuntime;
use rind_base::transport::TransportRuntime;
use rind_base::units::UnitsOrchestrator;
use rind_base::user::UserRuntime;
use rind_core::{notifier::Notifier, prelude::*};
use rind_plugins::{collect_plugins, plugins_path};
use std::os::fd::AsFd;

struct BootOrchestrator;

impl Orchestrator for BootOrchestrator {
  fn id(&self) -> &str {
    "boot"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("mounts", "mount_all", Default::default())?;

    ctx.dispatch("user", "create_sessions", Default::default())?;

    ctx.dispatch("sockets", "watch_events", Default::default())?;
    ctx.dispatch("services", "watch_events", Default::default())?;

    ctx.dispatch("ipc", "init_actions", Default::default())?;
    ctx.dispatch("ipc", "start_server", Default::default())?;

    ctx.dispatch("flow", "bootstrap", Default::default())?;
    ctx.dispatch("sockets", "bootstrap", Default::default())?;
    ctx.dispatch("services", "bootstrap", Default::default())?;
    ctx.dispatch("networking", "bootstrap", Default::default())?;
    ctx.dispatch("networking", "scan", Default::default())?;
    ctx.dispatch("networking", "configure", Default::default())?;

    ctx.dispatch("sockets", "setup_all", Default::default())?;
    ctx.dispatch("services", "start_all", Default::default())?;
    ctx.dispatch("services", "evaluate_triggers", Default::default())?;
    ctx.dispatch("sockets", "evaluate_triggers", Default::default())?;

    Ok(())
  }
}

struct RuntimeProviderOrchestrator;

impl Orchestrator for RuntimeProviderOrchestrator {
  fn id(&self) -> &str {
    "runtime-provider"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect],
      phase: BootPhase::Start,
    }
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![
      Box::new(ServiceRuntime::default()),
      Box::new(MountRuntime::default()),
      Box::new(FlowRuntime::default()),
      Box::new(TransportRuntime::default()),
      Box::new(NetworkingRuntime::default()),
      Box::new(ReaperRuntime::default()),
      Box::new(IpcRuntime::default()),
      Box::new(UserRuntime::default()),
      Box::new(SocketRuntime::default()),
      Box::new(TimerRuntime::default()),
    ]
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    Ok(())
  }
}

struct PumpOrchestrator;

impl Orchestrator for PumpOrchestrator {
  fn id(&self) -> &str {
    "pump"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Pump],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("reaper", "reap_once", Default::default())?;
    ctx.dispatch("reaper", "timeout_sweep", Default::default())?;
    ctx.dispatch("networking", "scan", Default::default())?;
    ctx.dispatch("networking", "reconcile", Default::default())?;
    ctx.dispatch("services", "drain_events", Default::default())?;
    ctx.dispatch("sockets", "drain_events", Default::default())?;
    ctx.dispatch("transport", "drain_incoming", Default::default())?;
    ctx.dispatch("ipc", "drain_requests", Default::default())?;
    Ok(())
  }
}

fn try_stop_services(
  boot: &BootEngine,
  metadata: &MetadataRegistry,
  runtime: &RuntimeHandle,
  resources: &mut Resources,
  force: bool,
) {
  let Some(context_id) = boot.primary_context_id() else {
    return;
  };

  let _ = runtime.dispatch(
    "services",
    "stop_all",
    rpayload!({ "force": force }),
    context_id,
  );
  let _ = runtime.flush_context(context_id, metadata, resources);
}

fn collect_other_pids() -> Vec<i32> {
  let self_pid = std::process::id() as i32;
  let mut pids = Vec::new();

  let Ok(entries) = std::fs::read_dir("/proc") else {
    return pids;
  };

  for entry in entries.flatten() {
    let name = entry.file_name();
    let name = name.to_string_lossy();
    let Ok(pid) = name.parse::<i32>() else {
      continue;
    };
    if pid <= 1 || pid == self_pid {
      continue;
    }
    pids.push(pid);
  }

  pids
}

fn terminate_all_processes() {
  let pids = collect_other_pids();
  for pid in &pids {
    let _ = kill(Pid::from_raw(*pid), Signal::SIGTERM);
  }

  std::thread::sleep(Duration::from_millis(500));

  let pids = collect_other_pids();
  for pid in &pids {
    let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
  }
}

fn load_env() {
  unsafe {
    for (key, value) in rind_core::utils::read_env_file("/etc/.env") {
      std::env::set_var(&key, &value);
    }
  }
}

fn process_lifecycle_action(
  action: LifecycleAction,
  boot: &mut BootEngine,
  metadata: &mut MetadataRegistry,
  instances: &mut InstanceMap,
  runtime: &RuntimeHandle,
  resources: &mut Resources,
) -> bool {
  match action {
    LifecycleAction::ReloadUnits => {
      load_env();
      let _ = boot.reload_units_collection(metadata, instances, runtime, resources);
      let _ = runtime.dispatch(
        "services",
        "bootstrap",
        Default::default(),
        boot.primary_context_id().unwrap_or(0),
      );
      let _ = runtime.flush_context(boot.primary_context_id().unwrap_or(0), metadata, resources);
      true
    }
    LifecycleAction::SoftReboot => {
      try_stop_services(boot, metadata, runtime, resources, false);
      terminate_all_processes();
      metadata.remove_metadata("units");
      let _ = boot.run(metadata, instances, runtime, resources);
      true
    }
    LifecycleAction::Reboot => {
      try_stop_services(boot, metadata, runtime, resources, false);
      terminate_all_processes();
      let _ = runtime.send(RuntimeCommand::Stop);
      unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
      }
      false
    }
    LifecycleAction::Shutdown => {
      try_stop_services(boot, metadata, runtime, resources, true);
      terminate_all_processes();
      let _ = runtime.send(RuntimeCommand::Stop);
      unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF);
      }
      false
    }
  }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  load_env();

  let units_dir = if let Ok(path) = std::env::var("RIND_UNITS_DIR") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/etc/units")
  };

  let pump_interval: u64 = std::env::var("RIND_PUMP_INTERVAL")
    .unwrap_or(15.to_string())
    .parse()
    .unwrap_or(15);

  let mut boot = BootEngine::default();
  let mut extensions = ExtensionManager::default();

  let units = UnitsOrchestrator::new(units_dir);

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let mut resources = Resources::default();

  let log = boot.start_logger();

  for plugin in collect_plugins(plugins_path(), &log)? {
    boot.orchestrators.extend(plugin.provide_orchestrators());
    plugin.register_extensions(&mut extensions);
  }
  EXTENSIONS.with(|e| match e.set(extensions) {
    Ok(_) => {}
    Err(_) => log.log(
      LogLevel::Error,
      "boot",
      "failed to allocate extensions",
      Default::default(),
    ),
  });

  boot.orchestrators.insert(0, PumpOrchestrator);
  boot.orchestrators.insert(0, BootOrchestrator);
  boot.orchestrators.insert(0, units);
  boot.orchestrators.insert(0, RuntimeProviderOrchestrator);

  let notifier = Notifier::new().expect("failed to create notifier");
  let runtime = boot.init_runtime(log.clone(), Some(notifier.clone()));

  boot
    .run(&mut metadata, &mut instances, &runtime, &mut resources)
    .map_err(|e| format!("boot failed: {e}"))?;

  let context_id = boot
    .primary_context_id()
    .ok_or_else(|| "missing runtime context id after boot".to_string())?;

  let mut sigset = SigSet::empty();
  sigset.add(Signal::SIGCHLD);
  sigset.thread_block().expect("failed to block SIGCHLD");

  let sfd = SignalFd::with_flags(&sigset, SfdFlags::SFD_NONBLOCK | SfdFlags::SFD_CLOEXEC)
    .expect("failed to create signalfd");

  let tfd = TimerFd::new(
    ClockId::CLOCK_MONOTONIC,
    TimerFlags::TFD_NONBLOCK | TimerFlags::TFD_CLOEXEC,
  )
  .expect("failed to create timerfd");

  tfd
    .set(
      Expiration::Interval(TimeSpec::from(Duration::from_secs(pump_interval))),
      TimerSetTimeFlags::empty(),
    )
    .expect("failed to set timerfd");

  let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC).expect("failed to create epoll");

  // 0 = Notifier, 1 = SignalFd, 2 = TimerFd
  epoll
    .add(notifier.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 0))
    .expect("failed to add notifier to epoll");
  epoll
    .add(sfd.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 1))
    .expect("failed to add signalfd to epoll");
  epoll
    .add(tfd.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 2))
    .expect("failed to add timerfd to epoll");

  let mut events = [EpollEvent::empty(); 16];

  loop {
    while let Some(action) = runtime.next_lifecycle_action(context_id) {
      if !process_lifecycle_action(
        action,
        &mut boot,
        &mut metadata,
        &mut instances,
        &runtime,
        &mut resources,
      ) {
        return Ok(());
      }
    }

    for fd in resources.removed_fds() {
      use std::os::fd::BorrowedFd;

      let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };

      match epoll.delete(borrowed) {
        Err(e) => log.log(
          LogLevel::Error,
          "epoll",
          &format!("failed to delete dynamic resource \"{fd}\": {e}"),
          Default::default(),
        ),
        _ => {}
      }

      if !resources.is_paused(fd) {
        resources.remove_full(fd);
      } else {
        resources.clear_removed(fd);
      }
    }

    for fd in resources.unwatched_fds() {
      use std::os::fd::BorrowedFd;

      let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };

      match epoll.add(
        borrowed,
        EpollEvent::new(resources.flags(fd), fd as u64 + 100),
      ) {
        Ok(_) | Err(nix::Error::EEXIST) => {
          resources.watch(fd);
        }
        Err(e) => log.log(
          LogLevel::Error,
          "epoll",
          &format!("failed to add dynamic resource \"{fd}\": {e}"),
          Default::default(),
        ),
      }
    }

    let n = epoll
      .wait(&mut events, nix::sys::epoll::EpollTimeout::NONE)
      .expect("epoll_wait failed");
    // println!("Here");

    for i in 0..n {
      let event = events[i];
      match event.data() {
        0 => {
          notifier.reset().ok();
        }
        1 => {
          let _ = sfd.read_signal();
          // quick fix
          let _ = runtime.dispatch("reaper", "reap_once", Default::default(), context_id);
        }
        2 => {
          // println!("here");
          let _ = tfd.wait();
        }
        d if d >= 100 => {
          let fd = (d - 100) as i32;
          if let Some(act) = resources.get_action(fd) {
            let payload = RuntimePayload::default().insert("fd", fd);
            let _ = runtime.dispatch(
              &act.runtime,
              &act.action,
              if let Some(p) = &act.payload {
                p(payload)
              } else {
                payload
              },
              context_id,
            );
          }
        }
        _ => {}
      }
    }

    // println!("Pumping");
    let _ = boot.pump_once(&mut metadata, &mut instances, &runtime, &mut resources);
  }
}
