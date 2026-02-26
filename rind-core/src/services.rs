use crate::name::Name;
use crate::units::UNITS;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use std::process::{Child, Command};

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub enum ServiceState {
  Active,
  #[default]
  Inactive,
  Exited(i32),
  Error(String),
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Service {
  pub name: String,
  pub exec: String,
  pub args: Vec<String>,
  pub restart: bool,

  #[serde(skip, default)]
  pub child: Option<Child>,

  #[serde(default)]
  pub last_state: ServiceState,
}

impl crate::units::UnitComponent for Service {
  fn find_in_unit<'a>(unit: &'a crate::units::Unit, name: &str) -> Option<&'a Self> {
    unit.service.as_ref()?.iter().find(|s| s.name == name)
  }
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
pub struct Socket(pub u32);

pub fn spawn_service(service: &mut Service) -> anyhow::Result<()> {
  let child = Command::new(&service.exec).args(&service.args).spawn()?;

  println!("Started service {} with PID {}", service.name, child.id());
  service.child = Some(child);
  Ok(())
}

pub fn start_service(service: &mut Service) {
  match spawn_service(service) {
    Ok(_) => service.last_state = ServiceState::Active,
    Err(e) => {
      let err = format!("Failed to start service \"{}\": {e}", service.name);
      println!("{err}");
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
  for unit in units.enabled_mut() {
    if let Some(ref mut services) = unit.service {
      for service in services {
        start_service(service);
      }
    }
  }
}

pub fn service_loop() {
  loop {
    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
      Ok(WaitStatus::Exited(pid, code)) => {
        // println!("Child {} exited with code {}", pid, code);

        let mut units = UNITS.write().unwrap();
        let mut to_restart = vec![];

        for (name, service) in units.services_mut() {
          if let Some(child) = &service.child {
            if child.id() as i32 == pid.as_raw() {
              service.last_state = ServiceState::Exited(code);
              service.child = None;
              if service.restart {
                to_restart.push(name.clone());
              }
            }
          }
        }

        drop(units);
        for name in to_restart {
          let mut units = UNITS.write().unwrap();
          let mut services = units
            .services_mut()
            .collect::<HashMap<&Name, &mut Service>>();

          if let Some(service) = services.get_mut(&name) {
            start_service(service);
          }
        }
      }
      Ok(_) => {}
      Err(e) => eprintln!("waitpid error: {}", e),
    }

    thread::sleep(Duration::from_millis(100));
  }
}
