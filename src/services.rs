use crate::names::Name;
use crate::units::{UNITS, Units};
use libc::VM_VFS_CACHE_PRESSURE;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use std::{fs, thread};

use std::process::{Child, Command};

#[derive(serde::Deserialize)]
pub struct Service {
  pub name: String,
  pub exec: String,
  pub args: Vec<String>,
  pub restart: bool,

  #[serde(skip, default)]
  pub child: Option<Child>,
}

impl crate::units::UnitComponent for Service {
  fn find_in_unit<'a>(unit: &'a crate::units::Unit, name: &str) -> Option<&'a Self> {
    unit.service.as_ref()?.iter().find(|s| s.name == name)
  }
}

#[derive(serde::Deserialize, Default)]
pub struct Socket(pub u32);

pub fn spawn_service(service: &mut Service) {
  let child = Command::new(&service.exec)
    .args(&service.args)
    .spawn()
    .unwrap();

  println!("Started service {} with PID {}", service.name, child.id());
  service.child = Some(child);
}

pub fn start_services() {
  let mut units = UNITS.write().unwrap();
  for unit in units.enabled_mut() {
    if let Some(ref mut services) = unit.service {
      for service in services {
        spawn_service(service);
      }
    }
  }
}

pub fn service_loop() {
  loop {
    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
      Ok(WaitStatus::Exited(pid, code)) => {
        println!("Child {} exited with code {}", pid, code);

        let mut units = UNITS.write().unwrap();
        let mut to_restart = vec![];

        for (name, service) in units.services_mut() {
          if let Some(child) = &service.child {
            if child.id() as i32 == pid.as_raw() {
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
            spawn_service(service);
          }
        }
      }
      Ok(_) => {}
      Err(e) => eprintln!("waitpid error: {}", e),
    }

    thread::sleep(Duration::from_millis(100));
  }
}
