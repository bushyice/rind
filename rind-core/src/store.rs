use crate::lookup::ComponentFilter;
use crate::mount::{mount_target, umount_target};
use crate::name::Name;
use crate::services::{start_service, stop_service};
use crate::units::Unit;
use once_cell::sync::Lazy;
use std::collections::HashMap;

pub static STORE: Lazy<std::sync::RwLock<Store>> =
  Lazy::new(|| std::sync::RwLock::new(Store::default()));

#[derive(Default)]
pub struct Store {
  pub(crate) units: HashMap<Name, Unit>,
  pub(crate) enabled: HashMap<Name, ComponentFilter>,
}

impl Store {
  pub fn insert_unit(&mut self, name: impl Into<Name>, mut unit: Unit) {
    let name = name.into();
    unit.build_index(&name);
    self.units.insert(name, unit);
  }

  pub fn enable_unit(&mut self, name: impl Into<Name>, write: bool) {
    let name = name.into();
    let mut filter = self.enabled.get(&name).cloned().unwrap_or_default();
    filter.include.clear();
    filter.exclude.clear();

    if let Some(unit) = self.units.get_mut(&name) {
      if let Some(ref mut services) = unit.service {
        for svc in services {
          if filter.include.is_empty() || filter.include.contains(&svc.name) {
            if !filter.exclude.contains(&svc.name) {
              start_service(svc);
            }
          }
        }
      }

      if let Some(ref mounts) = unit.mount {
        for mount in mounts {
          let mname = &mount.target;
          if filter.include.is_empty() || filter.include.contains(mname) {
            if !filter.exclude.contains(mname) {
              mount_target(mount);
            }
          }
        }
      }

      if let Some(ref sockets) = unit.socket {
        for socket in sockets {
          let sname = &socket.name;
          if filter.include.is_empty() || filter.include.contains(sname) {
            if !filter.exclude.contains(sname) {
              // start_socket(socket);
            }
          }
        }
      }
    }

    if write {
      self.save_enabled();
    }
  }

  pub fn disable_unit(&mut self, name: impl Into<Name>, write: bool) {
    let name = name.into();
    if let Some(ref mut unit) = self.units.get_mut(&name) {
      if let Some(ref mut services) = unit.service {
        for service in services {
          stop_service(service, true);
        }
      }

      if let Some(ref mounts) = unit.mount {
        for mount in mounts {
          umount_target(mount);
        }
      }
    }

    self.enabled.remove(&name);
    if write {
      self.save_enabled();
    }
  }

  pub fn enable_component(&mut self, unit_name: impl Into<Name>, component: &str, write: bool) {
    let unit_name = unit_name.into();
    let filter = self.enabled.entry(unit_name.clone()).or_default();
    filter.include.insert(component.to_string());
    // filter.exclude.remove(component);

    if let Some(unit) = self.units.get_mut(&unit_name) {
      if let Some(services) = &mut unit.service {
        for svc in services {
          if svc.name == component {
            start_service(svc);
          }
        }
      }
      if let Some(mounts) = &unit.mount {
        for mount in mounts {
          if mount.target == component {
            mount_target(mount);
          }
        }
      }
      if let Some(sockets) = &unit.socket {
        for socket in sockets {
          if socket.name == component {
            // start_socket(socket);
          }
        }
      }
    }

    if write {
      self.save_enabled();
    }
  }

  pub fn disable_component(&mut self, unit_name: impl Into<Name>, component: &str, write: bool) {
    let unit_name = unit_name.into();
    let filter = self.enabled.entry(unit_name.clone()).or_default();
    // filter.exclude.insert(component.to_string());
    filter.include.remove(component);

    if let Some(unit) = self.units.get_mut(&unit_name) {
      if let Some(services) = &mut unit.service {
        for svc in services {
          if svc.name == component {
            stop_service(svc, true);
          }
        }
      }
      if let Some(mounts) = &unit.mount {
        for mount in mounts {
          if mount.target == component {
            umount_target(mount);
          }
        }
      }
      if let Some(sockets) = &unit.socket {
        for socket in sockets {
          if socket.name == component {
            // stop_socket(socket);
          }
        }
      }
    }

    if write {
      self.save_enabled();
    }
  }

  pub fn each(&self) -> impl Iterator<Item = (&Name, &Unit)> {
    self.units.iter()
  }

  // pub fn enabled(&self) -> impl Iterator<Item = &Unit> {
  //   self.units.iter().filter_map(move |(name, unit)| {
  //     if self.enabled.contains_key(name) {
  //       Some(unit)
  //     } else {
  //       None
  //     }
  //   })
  // }
  // pub fn enabled_mut(&mut self) -> impl Iterator<Item = &mut Unit> {
  //   self.units.iter_mut().filter_map(|(name, unit)| {
  //     if self.enabled.contains_key(name) {
  //       Some(unit)
  //     } else {
  //       None
  //     }
  //   })
  // }

  // pub fn enabled_services(&self) -> impl Iterator<Item = &Service> {
  //   self.units.iter().flat_map(move |(unit_name, unit)| {
  //     let filter = self.enabled.get(unit_name);

  //     filter_enabled!(unit.service.iter().flat_map(|s| s.iter()), filter)
  //   })
  // }

  pub fn parse_enabled(&mut self, content: &str) {
    for line in content.lines().map(str::trim).filter(|x| !x.is_empty()) {
      if let Some((unit_name, rest)) = line.split_once('@') {
        let mut filter = ComponentFilter::default();
        if let Some(inner) = rest.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
          for item in inner.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            if let Some(stripped) = item.strip_prefix('!') {
              filter.exclude.insert(stripped.to_string());
            } else {
              filter.include.insert(item.to_string());
            }
          }
        }
        self.enabled.insert(unit_name.into(), filter);
      } else {
        self.enabled.insert(line.into(), ComponentFilter::default());
      }
    }
  }

  pub fn save_enabled(&self) {
    let enabled_path = std::path::PathBuf::from(
      rind_common::config::CONFIG
        .read()
        .unwrap()
        .services
        .path
        .as_str(),
    )
    .join(".enabled");

    let mut lines = vec![];
    for (name, filter) in &self.enabled {
      if filter.include.is_empty() && filter.exclude.is_empty() {
        lines.push(name.to_string());
      } else {
        let mut parts = vec![];
        for inc in &filter.include {
          parts.push(inc.clone());
        }
        for exc in &filter.exclude {
          parts.push(format!("!{}", exc));
        }
        lines.push(format!("{}@{{{}}}", name.to_string(), parts.join(",")));
      }
    }

    std::fs::write(enabled_path, lines.join("\n")).unwrap();
  }

  pub fn unit(&self, name: impl Into<Name>) -> Option<&Unit> {
    self.units.get(&name.into())
  }

  pub fn unit_mut(&mut self, name: impl Into<Name>) -> Option<&mut Unit> {
    self.units.get_mut(&name.into())
  }

  pub fn names(&self) -> impl Iterator<Item = &Name> {
    self.units.keys()
  }

  pub fn units(&self) -> impl Iterator<Item = &Unit> {
    self.units.values()
  }

  pub fn iter(&self) -> impl Iterator<Item = (&Name, &Unit)> {
    self.units.iter()
  }

  pub fn enabled_names(&self) -> impl Iterator<Item = &Name> {
    self.enabled.iter().map(|x| x.0)
  }

  pub fn enabled_get(&self, name: &Name) -> Option<&ComponentFilter> {
    self.enabled.get(name)
  }
}
