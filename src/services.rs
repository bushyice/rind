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

#[derive(serde::Deserialize, Default)]
pub struct Socket(pub u32);

#[derive(serde::Deserialize)]
struct ServicesRoot {
  service: Option<Vec<Service>>,
  socket: Option<Vec<Socket>>,
}

pub static SERVICES: Lazy<std::sync::RwLock<HashMap<String, Service>>> =
  Lazy::new(|| std::sync::RwLock::new(HashMap::new()));

pub static ENABLED_SERVICES: Lazy<std::sync::Mutex<Vec<String>>> =
  Lazy::new(|| std::sync::Mutex::new(Vec::new()));

pub fn load_services_from(path: &str) -> Result<HashMap<String, Service>, anyhow::Error> {
  let mut services = HashMap::new();

  for entry in
    fs::read_dir(path).map_err(|e| anyhow::anyhow!("Failed to read services folder: {e}"))?
  {
    let entry = entry?;
    let path = entry.path();

    if entry.file_type()?.is_file() && path.extension().map_or(false, |x| x == "toml") {
      let content =
        fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read unit: {e}"))?;
      let parsed: ServicesRoot = toml::from_str(&content)?;

      for service in parsed.service.unwrap() {
        services.insert(service.name.clone(), service);
      }
    }
  }

  Ok(services)
}

pub fn spawn_service(service: &mut Service) {
  let child = Command::new(&service.exec)
    .args(&service.args)
    .spawn()
    .unwrap();

  println!("Started service {} with PID {}", service.name, child.id());
  service.child = Some(child);
}

pub fn load_services() -> Result<(), anyhow::Error> {
  let loaded_services = load_services_from(&crate::config::CONFIG.lock().unwrap().services.path)?;
  let mut services = SERVICES.write().unwrap();
  *services = loaded_services;
  start_services(services.values_mut().collect());
  Ok(())
}

pub fn start_services(services: Vec<&mut Service>) {
  let enabled = ENABLED_SERVICES.lock().unwrap();
  for service in services.into_iter() {
    if enabled.contains(&service.name) {
      spawn_service(service);
    }
  }
}

pub fn service_loop() {
  loop {
    match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
      Ok(WaitStatus::Exited(pid, code)) => {
        println!("Child {} exited with code {}", pid, code);

        let mut services = SERVICES.write().unwrap();
        let mut to_restart = vec![];

        for (name, service) in services.iter_mut() {
          if let Some(child) = &service.child {
            if child.id() as i32 == pid.as_raw() {
              service.child = None;
              if service.restart {
                to_restart.push(name.clone());
              }
            }
          }
        }

        drop(services);
        for name in to_restart {
          let mut services = SERVICES.write().unwrap();
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
