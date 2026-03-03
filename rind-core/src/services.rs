use crate::name::Name;
use crate::store::STORE;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use rind_common::logger::{LOGGER, log_child};
use rind_common::{logerr, loginfo};
use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub enum ServiceState {
  Active,
  #[default]
  Inactive,
  Exited(i32),
  Error(String),
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
  pub restart: bool,
  pub after: Option<String>,

  #[serde(skip, default)]
  pub child: Option<Child>,

  #[serde(default)]
  pub last_state: ServiceState,
}

pub fn spawn_service(service: &mut Service) -> anyhow::Result<()> {
  let unit_name = service.unit.to_string();

  let mut child = Command::new(&service.exec)
    .args(&service.args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;

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
  match spawn_service(service) {
    Ok(_) => service.last_state = ServiceState::Active,
    Err(e) => {
      let err = format!("Failed to start service \"{}\": {e}", service.name);
      logerr!("{err}");
      service.last_state = ServiceState::Error(err);
    }
  }
}

pub fn stop_service(service: &mut Service, force: bool) {
  if let Some(child) = &mut service.child {
    if force {
      let pid = Pid::from_raw(child.id() as i32);
      kill(pid, Signal::SIGKILL).unwrap();
    } else {
      child.kill().unwrap();
    }
  }
  service.last_state = ServiceState::Inactive;
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

pub fn service_loop() {
  loop {
    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
      Ok(WaitStatus::Exited(pid, code)) => {
        loginfo!("Child {} exited with code {}", pid, code);

        let mut store = STORE.write().unwrap();
        let mut to_restart = vec![];

        for (_, service) in store.items_mut::<Service>() {
          if let Some(child) = &service.child {
            if child.id() as i32 == pid.as_raw() {
              service.last_state = ServiceState::Exited(code);
              service.child = None;
              if service.restart {
                to_restart.push(service.name.clone());
              }
            }
          }
        }

        drop(store);
        for name in to_restart {
          let mut store = STORE.write().unwrap();
          let mut services = store.items_mut::<Service>();

          if let Some(service) = services.find(|ser| ser.1.name == name).map(|x| x.1) {
            start_service(service);
          }
        }
      }
      Ok(_) => {}
      Err(e) => logerr!("waitpid error: {}", e),
    }

    thread::sleep(Duration::from_millis(100));
  }
}
