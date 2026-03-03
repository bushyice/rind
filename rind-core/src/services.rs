use crate::name::Name;
use crate::store::STORE;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use rind_common::logger::{LOGGER, log_child};
use rind_common::{logerr, loginfo};
use std::collections::HashSet;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use wait_timeout::ChildExt;

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub enum ServiceState {
  Active,
  #[default]
  Inactive,
  Starting,
  Stopping,
  Exited(i32),
  Error(String),
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum RestartPolicy {
  Bool(bool),
  OnFailure { max_retries: u32 },
}

impl Default for RestartPolicy {
  fn default() -> Self {
    Self::Bool(false)
  }
}

static SERVICE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceId(u64);

impl Default for ServiceId {
  fn default() -> Self {
    Self(SERVICE_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
  }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Service {
  #[serde(skip, default)]
  pub id: ServiceId,
  #[serde(skip, default)]
  pub unit: Name,

  pub name: String,
  pub exec: String,
  pub args: Vec<String>,
  pub after: Option<String>,

  #[serde(default)]
  pub restart: RestartPolicy,

  #[serde(skip, default)]
  pub child: Option<Child>,

  #[serde(default)]
  pub state: ServiceState,

  #[serde(skip, default)]
  pub retry_count: u32,

  #[serde(skip)]
  pub manually_stopped: bool,
}

pub fn spawn_service(service: &mut Service) -> anyhow::Result<()> {
  let unit_name = service.unit.to_string();

  let mut child = unsafe {
    Command::new(&service.exec)
      .args(&service.args)
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .pre_exec(|| {
        libc::setsid();
        Ok(())
      })
      .spawn()?
  };

  log_child(
    &mut child,
    if !unit_name.is_empty() {
      format!("{}@{}", unit_name, service.name)
    } else {
      service.name.clone()
    },
    LOGGER.clone(),
  );

  loginfo!("Started service {} with PID {}", service.name, child.id());
  service.child = Some(child);
  Ok(())
}

pub fn start_service(service: &mut Service) {
  service.state = ServiceState::Starting;
  match spawn_service(service) {
    Ok(_) => service.state = ServiceState::Active,
    Err(e) => {
      let err = format!("Failed to start service \"{}\": {e}", service.name);
      logerr!("{err}");
      service.state = ServiceState::Error(err);
    }
  }
}

pub fn stop_service(service: &mut Service, force: bool) {
  service.state = ServiceState::Stopping;

  if let Some(mut child) = service.child.take() {
    let pgid = Pid::from_raw(-(child.id() as i32));

    let signal = if force {
      Signal::SIGKILL
    } else {
      Signal::SIGTERM
    };

    let _ = kill(pgid, signal);

    service.manually_stopped = true;

    match child.wait_timeout(Duration::from_secs(5)) {
      Ok(Some(status)) => {
        service.state = ServiceState::Exited(status.code().unwrap_or(-1));
      }
      Ok(None) => {
        let _ = kill(pgid, Signal::SIGKILL);
        let _ = child.wait();
        service.state = ServiceState::Exited(-1);
      }
      Err(_) => {
        service.state = ServiceState::Error("wait_timeout failed".into());
      }
    }
  } else {
    service.state = ServiceState::Inactive;
  }
}

pub fn start_services() {
  let mut store = STORE.write().unwrap();

  let mut started: HashSet<String> = HashSet::new();
  let mut pending = Vec::new();

  for (unit_name, service) in store.enabled_mut::<Service>() {
    let id = format!("{}@{}", unit_name.to_string(), service.name);
    if let Some(after) = &service.after {
      pending.push((unit_name.clone(), service.name.clone(), after.clone()));
    } else {
      start_service(service);
      started.insert(id);
    }
  }

  loop {
    let mut progress = false;

    pending.retain(|(_, service_name, after)| {
      if started.contains(after) {
        if let Some(service) = store.lookup_mut::<Service>(service_name) {
          start_service(service);
          started.insert(service_name.clone());
          progress = true;
        }
        false
      } else {
        true
      }
    });

    if !progress {
      break;
    }
  }

  if !pending.is_empty() {
    logerr!(
      "Unresolved dependencies: {:?}",
      pending
        .iter()
        .map(|x| format!("{}@{} for {}", x.0.to_string(), x.1, x.2))
        .collect::<Vec<String>>()
    );
  }
}

fn handle_exit(pid: Pid, code: i32) {
  loginfo!("Child {} exited with code {}", pid, code);

  let mut to_restart: Vec<ServiceId> = Vec::new();

  {
    let mut store = STORE.write().unwrap();

    for (_, service) in store.items_mut::<Service>() {
      if let Some(child) = &service.child {
        if child.id() as i32 == pid.as_raw() {
          service.state = ServiceState::Exited(code);
          service.child = None;

          if service.manually_stopped {
            continue;
          }

          match service.restart {
            RestartPolicy::Bool(false) => {}

            RestartPolicy::Bool(true) => {
              to_restart.push(service.id);
            }

            RestartPolicy::OnFailure { max_retries } => {
              if code != 0 && max_retries > 0 && service.retry_count < max_retries {
                to_restart.push(service.id);
                service.retry_count += 1;
              }
            }
          }
        }
      }
    }
  }

  if !to_restart.is_empty() {
    let mut store = STORE.write().unwrap();

    for (_, service) in store.items_mut::<Service>() {
      if to_restart.contains(&service.id) {
        start_service(service);
      }
    }
  }
}

pub fn service_loop() {
  loop {
    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
      Ok(WaitStatus::Exited(pid, code)) => {
        handle_exit(pid, code);
      }

      Ok(WaitStatus::Signaled(pid, signal, _)) => {
        let code = 128 + signal as i32;
        handle_exit(pid, code);
      }

      Ok(WaitStatus::StillAlive) => {}

      Ok(_) => {}

      Err(nix::errno::Errno::ECHILD) => {}

      Err(e) => {
        logerr!("waitpid error: {}", e);
      }
    }

    thread::sleep(Duration::from_millis(100));
  }
}
