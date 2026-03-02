use crate::logger::{LOGGER, log_child};
use crate::units::UNITS;
use crate::{logerr, loginfo};
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Default, serde::Serialize, serde::Deserialize)]
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

  pub name: String,
  pub exec: String,
  pub args: Vec<String>,
  pub restart: bool,

  #[serde(skip, default)]
  pub child: Option<Child>,

  #[serde(default)]
  pub last_state: ServiceState,
}

pub fn spawn_service(service: &mut Service) -> anyhow::Result<()> {
  let mut child = Command::new(&service.exec).args(&service.args).spawn()?;

  log_child(&mut child, &service, LOGGER.clone());

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
  let mut units = UNITS.write().unwrap();
  for service in units.enabled_mut::<Service>() {
    start_service(service);
  }
}

pub fn service_loop() {
  loop {
    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
      Ok(WaitStatus::Exited(pid, code)) => {
        loginfo!("Child {} exited with code {}", pid, code);

        let mut units = UNITS.write().unwrap();
        let mut to_restart = vec![];

        for service in units.items_mut::<Service>() {
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

        drop(units);
        for name in to_restart {
          let mut units = UNITS.write().unwrap();
          let mut services = units.items_mut::<Service>();

          if let Some(service) = services.find(|ser| ser.name == name) {
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
