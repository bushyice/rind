use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags};
use nix::sys::signal::{Signal, kill};
use nix::sys::signalfd::{SfdFlags, SigSet, SignalFd};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use nix::unistd::Pid;

use std::os::fd::AsFd;
use std::time::Duration;

use rind_core::prelude::*;

pub fn setup_event_loop(
  boot: &mut BootEngine,
  metadata: &mut MetadataRegistry,
  instances: &mut InstanceMap,
  resources: &mut Resources,
  log: &LogHandle,
  pump_interval: u64,
) -> Result<EventLoop, Box<dyn std::error::Error>> {
  let notifier = Notifier::new().map_err(|e| format!("failed to create notifier: {e}"))?;
  let runtime = boot.init_runtime(log.clone(), Some(notifier.clone()));

  boot
    .run(metadata, instances, &runtime, resources)
    .map_err(|e| format!("boot failed: {e}"))?;

  let context_id = boot
    .primary_context_id()
    .ok_or("missing runtime context id after boot")?;

  let mut sigset = SigSet::empty();
  sigset.add(Signal::SIGCHLD);
  sigset
    .thread_block()
    .map_err(|e| format!("failed to block SIGCHLD: {e}"))?;

  let sfd = SignalFd::with_flags(&sigset, SfdFlags::SFD_NONBLOCK | SfdFlags::SFD_CLOEXEC)
    .map_err(|e| format!("failed to create signalfd: {e}"))?;

  let tfd = TimerFd::new(
    ClockId::CLOCK_MONOTONIC,
    TimerFlags::TFD_NONBLOCK | TimerFlags::TFD_CLOEXEC,
  )
  .map_err(|e| format!("failed to create timerfd: {e}"))?;

  tfd
    .set(
      Expiration::Interval(TimeSpec::from(Duration::from_secs(pump_interval))),
      TimerSetTimeFlags::empty(),
    )
    .map_err(|e| format!("failed to set timerfd: {e}"))?;

  let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)
    .map_err(|e| format!("failed to create epoll: {e}"))?;

  epoll
    .add(notifier.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 0))
    .map_err(|e| format!("failed to add notifier to epoll: {e}"))?;
  epoll
    .add(sfd.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 1))
    .map_err(|e| format!("failed to add signalfd to epoll: {e}"))?;
  epoll
    .add(tfd.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, 2))
    .map_err(|e| format!("failed to add timerfd to epoll: {e}"))?;

  Ok(EventLoop {
    runtime,
    context_id,
    notifier,
    sfd,
    tfd,
    epoll,
    log: log.clone(),
  })
}

pub struct EventLoop {
  runtime: RuntimeHandle,
  context_id: usize,
  notifier: Notifier,
  sfd: SignalFd,
  tfd: TimerFd,
  epoll: Epoll,
  log: LogHandle,
}

impl EventLoop {
  pub fn run(
    &self,
    boot: &mut BootEngine,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    resources: &mut Resources,
  ) -> bool {
    let mut events = [EpollEvent::empty(); 16];

    loop {
      while let Some(action) = self.runtime.next_lifecycle_action(self.context_id) {
        if !process_lifecycle_action(action, boot, metadata, instances, &self.runtime, resources) {
          return false;
        }
      }

      for fd in resources.removed_fds() {
        use std::os::fd::BorrowedFd;

        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };

        match self.epoll.delete(borrowed) {
          Err(e) => self.log.log(
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

        match self.epoll.add(
          borrowed,
          EpollEvent::new(resources.flags(fd), fd as u64 + 100),
        ) {
          Ok(_) | Err(nix::Error::EEXIST) => {
            resources.watch(fd);
          }
          Err(e) => self.log.log(
            LogLevel::Error,
            "epoll",
            &format!("failed to add dynamic resource \"{fd}\": {e}"),
            Default::default(),
          ),
        }
      }

      let n = match self
        .epoll
        .wait(&mut events, nix::sys::epoll::EpollTimeout::NONE)
      {
        Ok(n) => n,
        Err(nix::Error::EINTR) => continue,
        Err(e) => {
          self.log.log(
            LogLevel::Error,
            "epoll",
            &format!("epoll_wait failed: {e}"),
            Default::default(),
          );
          continue;
        }
      };

      for i in 0..n {
        let event = events[i];
        match event.data() {
          0 => {
            self.notifier.reset().ok();
          }
          1 => {
            let _ = self.sfd.read_signal();
            let _ =
              self
                .runtime
                .dispatch("reaper", "reap_once", Default::default(), self.context_id);
          }
          2 => {
            let _ = self.tfd.wait();
          }
          d if d >= 100 => {
            let fd = (d - 100) as i32;
            if let Some(act) = resources.get_action(fd) {
              let payload = RuntimePayload::default().insert("fd", fd);
              let _ = self.runtime.dispatch(
                &act.runtime,
                &act.action,
                if let Some(p) = &act.payload {
                  p(payload)
                } else {
                  payload
                },
                self.context_id,
              );
            }
          }
          _ => {}
        }
      }

      let _ = boot.pump_once(metadata, instances, &self.runtime, resources);
    }
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
      crate::early::load_env();
      let _ = boot.reload_units_collection(metadata, instances, runtime, resources);
      // let _ = runtime.dispatch(
      //   "services",
      //   "bootstrap",
      //   Default::default(),
      //   boot.primary_context_id().unwrap_or(0),
      // );
      // let _ = runtime.dispatch(
      //   "sockets",
      //   "bootstrap",
      //   Default::default(),
      //   boot.primary_context_id().unwrap_or(0),
      // );
      // let _ = runtime.dispatch(
      //   "services",
      //   "start_all",
      //   Default::default(),
      //   boot.primary_context_id().unwrap_or(0),
      // );
      // let _ = runtime.dispatch(
      //   "sockets",
      //   "setup_all",
      //   Default::default(),
      //   boot.primary_context_id().unwrap_or(0),
      // );
      let _ = runtime.dispatch(
        "events",
        "reload_scopes",
        Default::default(),
        boot.primary_context_id().unwrap_or(0),
      );
      let _ = runtime.flush_context(boot.primary_context_id().unwrap_or(0), metadata, resources);
      true
    }
    LifecycleAction::SoftReboot => {
      try_stop_services(boot, metadata, runtime, resources, false);
      terminate_all_processes();
      let _ = runtime.send(RuntimeCommand::Stop);
      unsafe {
        libc::sync();
        let exe = std::env::current_exe().expect("failed to get current exe");
        let exe_c =
          std::ffi::CString::new(exe.as_os_str().as_encoded_bytes()).expect("exe path has null");
        let argv0 = std::env::args().next().unwrap_or_default();
        let argv0_c = std::ffi::CString::new(argv0).expect("argv0 has null");
        let argv: [*const libc::c_char; 2] = [argv0_c.as_ptr(), std::ptr::null()];
        libc::execv(exe_c.as_ptr(), argv.as_ptr());
        libc::_exit(1);
      }
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
