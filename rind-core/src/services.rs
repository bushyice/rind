use crate::flow::{FlowItem, TransportMethod, TransportProtocol, Trigger};
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

#[derive(Default, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
  pub after: Option<Vec<String>>,

  #[serde(rename = "start-on")]
  pub start_on: Option<Vec<FlowItem>>,
  #[serde(rename = "stop-on")]
  pub stop_on: Option<Vec<FlowItem>>,
  #[serde(rename = "on-start")]
  pub on_start: Option<Vec<Trigger>>,
  #[serde(rename = "on-stop")]
  pub on_stop: Option<Vec<Trigger>>,
  #[serde(rename = "transport")]
  pub transport: Option<TransportMethod>,

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

  #[serde(skip)]
  pub stop_time: Option<std::time::Instant>,
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
    Ok(_) => {
      service.state = ServiceState::Active;
    }
    Err(e) => {
      let err = format!("Failed to start service \"{}\": {e}", service.name);
      logerr!("{err}");
      service.state = ServiceState::Error(err);
    }
  }
}

pub fn stop_service(service: &mut Service, force: bool) {
  service.state = ServiceState::Stopping;
  service.stop_time = Some(std::time::Instant::now());

  if let Some(child) = service.child.as_ref() {
    let pgid = Pid::from_raw(-(child.id() as i32));

    let signal = if force {
      Signal::SIGKILL
    } else {
      Signal::SIGTERM
    };

    let _ = kill(pgid, signal);
    service.manually_stopped = true;
  } else {
    service.state = ServiceState::Inactive;
  }
}

pub fn start_services() {
  let mut store = STORE.write().unwrap();

  let mut started: HashSet<String> = HashSet::new();
  let mut pending = Vec::new();

  for (_, service) in store.enabled_mut::<Service>() {
    let id = service.name.clone();
    if let Some(afters) = &service.after {
      pending.push((service.name.clone(), afters.clone()));
    } else {
      start_service(service);
      started.insert(id);
    }
  }

  loop {
    let mut progress = false;

    pending.retain(|(service_name, afters)| {
      if afters.iter().all(|a| started.contains(a)) {
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
        .map(|x| format!("{} for {:?}", x.0, x.1))
        .collect::<Vec<String>>()
    );
  }
}

pub fn start_dependents(store: &mut crate::store::Store, target: &str) {
  let mut to_start = Vec::new();

  for (unit_name, service) in store.enabled::<Service>() {
    if service.state == ServiceState::Inactive || service.state == ServiceState::Exited(0) {
      if let Some(afters) = &service.after {
        if afters.contains(&target.to_string()) {
          to_start.push(format!("{}@{}", unit_name.to_string(), service.name));
        }
      }
    }
  }

  for name in to_start {
    if let Some(service) = store.lookup_mut::<Service>(&name) {
      start_service(service);
      let t = service.name.clone();
      start_dependents(store, &t);
    }
  }
}

pub fn stop_dependents(store: &mut crate::store::Store, target: &str, force: bool) {
  let mut to_stop = Vec::new();

  for (unit_name, service) in store.items::<Service>() {
    if service.state == ServiceState::Active || service.state == ServiceState::Starting {
      if let Some(afters) = &service.after {
        if afters.contains(&target.to_string()) {
          to_stop.push(format!("{}@{}", unit_name.to_string(), service.name));
        }
      }
    }
  }

  for name in to_stop {
    if let Some(service) = store.lookup_mut::<Service>(&name) {
      stop_service(service, force);
      let t = service.name.clone();
      stop_dependents(store, &t, force);
    }
  }
}

fn handle_exit(pid: Pid, code: i32) {
  loginfo!("Child {} exited with code {}", pid, code);

  let mut to_restart: Vec<ServiceId> = Vec::new();
  let mut to_start_deps: Vec<String> = Vec::new();
  let mut to_stop_deps: Vec<String> = Vec::new();

  {
    let mut store = STORE.write().unwrap();

    for (_, service) in store.items_mut::<Service>() {
      if let Some(child) = &service.child {
        if child.id() as i32 == pid.as_raw() {
          service.state = ServiceState::Exited(code);
          service.child = None;

          if service.manually_stopped {
            to_stop_deps.push(service.name.clone());
            continue;
          }

          match service.restart {
            RestartPolicy::Bool(false) => {
              to_stop_deps.push(service.name.clone());
            }

            RestartPolicy::Bool(true) => {
              to_restart.push(service.id);
            }

            RestartPolicy::OnFailure { max_retries } => {
              if code != 0 && max_retries > 0 && service.retry_count < max_retries {
                to_restart.push(service.id);
                service.retry_count += 1;
              } else {
                to_stop_deps.push(service.name.clone());
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
        to_start_deps.push(service.name.clone());
      }
    }
  }

  if !to_start_deps.is_empty() || !to_stop_deps.is_empty() {
    let mut store = STORE.write().unwrap();

    for name in to_stop_deps {
      stop_dependents(&mut store, &name, false);
    }
    for name in to_start_deps {
      start_dependents(&mut store, &name);
    }
  }
}

pub fn service_loop() {
  loop {
    loop {
      match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::Exited(pid, code)) => {
          handle_exit(pid, code);
        }

        Ok(WaitStatus::Signaled(pid, signal, _)) => {
          let code = 128 + signal as i32;
          handle_exit(pid, code);
        }

        Ok(WaitStatus::StillAlive) | Err(nix::errno::Errno::ECHILD) => {
          break;
        }

        Ok(_) => {}

        Err(e) => {
          logerr!("waitpid error: {}", e);
          break;
        }
      }
    }

    {
      let mut store = STORE.write().unwrap();
      for (_, service) in store.items_mut::<Service>() {
        if service.state == ServiceState::Stopping {
          if let Some(stop_time) = service.stop_time {
            if stop_time.elapsed() > Duration::from_secs(5) {
              if let Some(child) = service.child.as_ref() {
                let pgid = Pid::from_raw(-(child.id() as i32));
                let _ = kill(pgid, Signal::SIGKILL);
              }
            }
          }
        }
      }
    }

    thread::sleep(Duration::from_millis(100));
  }
}
