use crate::flow::FlowInstance;
use crate::mount::{mount_target, umount_target};
use crate::name::Name;
use crate::services::{start_service, stop_service};
use crate::units::Unit;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};

pub static STORE: Lazy<std::sync::RwLock<Store>> =
  Lazy::new(|| std::sync::RwLock::new(Store::default()));

#[derive(Default)]
pub struct Store {
  pub(crate) units: HashMap<Name, Unit>,
  pub(crate) enabled: HashMap<Name, HashSet<String>>,

  pub(crate) states: HashMap<String, Vec<FlowInstance>>,
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
    filter.clear();
    // filter.exclude.clear();

    if let Some(unit) = self.units.get_mut(&name) {
      if let Some(ref mut services) = unit.service {
        for svc in services {
          if filter.is_empty() || filter.contains(&svc.name) {
            start_service(svc);
          }
        }
      }

      if let Some(ref mounts) = unit.mount {
        for mount in mounts {
          let mname = &mount.target;
          if filter.is_empty() || filter.contains(mname) {
            mount_target(mount);
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
    filter.insert(component.to_string());
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
    }

    if write {
      self.save_enabled();
    }
  }

  pub fn disable_component(&mut self, unit_name: impl Into<Name>, component: &str, write: bool) {
    let unit_name = unit_name.into();
    let filter = self.enabled.entry(unit_name.clone()).or_default();
    // filter.exclude.insert(component.to_string());
    filter.remove(component);

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

  pub fn load_enabled(&mut self) {
    for inst in self.states.get("active").unwrap_or(&Vec::new()) {
      let line = inst.payload.to_string();
      if let Some((unit_name, rest)) = line.split_once('@') {
        let mut filter = HashSet::default();
        if let Some(inner) = rest.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
          for item in inner.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            filter.insert(item.to_string());
          }
        }
        self.enabled.insert(unit_name.into(), filter);
      } else {
        self.enabled.insert(line.into(), HashSet::default());
      }
    }
  }

  pub fn save_enabled(&mut self) {
    let mut lines = vec![];
    for (name, filter) in &self.enabled {
      if filter.is_empty() {
        lines.push(name.to_string());
      } else {
        let mut parts = vec![];
        for inc in filter {
          parts.push(inc.clone());
        }
        lines.push(format!("{}@{{{}}}", name.to_string(), parts.join(",")));
      }
    }

    self
      .states
      .get_mut("active")
      .unwrap_or(&mut Vec::new())
      .retain(|instance| lines.contains(&instance.payload.to_string()));
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

  pub fn enabled_get(&self, name: &Name) -> Option<&HashSet<String>> {
    self.enabled.get(name)
  }

  pub fn len(&self) -> usize {
    self.units.len()
  }

  pub fn state_branches(&self, name: &str) -> Option<&Vec<FlowInstance>> {
    self.states.get(name)
  }

  pub fn load_state(&mut self) {
    let config = rind_common::config::CONFIG.read().unwrap();
    let state_path = std::path::Path::new(config.units.state.as_str());
    if let Ok(content) = std::fs::read(&state_path) {
      self.states =
        bincode_next::serde::decode_from_slice(&content, bincode_next::config::standard())
          .unwrap()
          .0;
      if let Ok((states, _)) =
        bincode_next::serde::decode_from_slice(&content, bincode_next::config::standard())
      {
        self.states = states;
      } else {
        panic!("fuck")
      }
    }
  }

  pub fn save_state(&self) {
    let config = rind_common::config::CONFIG.read().unwrap();
    let state_path = std::path::Path::new(config.units.state.as_str());

    bincode_next::serde::encode_to_vec(&self.states, bincode_next::config::standard()).unwrap();

    if let Ok(serialized) =
      bincode_next::serde::encode_to_vec(&self.states, bincode_next::config::standard())
    {
      use std::io::Write;
      use std::os::unix::fs::OpenOptionsExt;
      if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&state_path)
      {
        let _ = f.write_all(&serialized);
      }
    }
  }
}
