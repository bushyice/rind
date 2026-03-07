use crate::flow::{FlowInstance, FlowPayload};
use crate::mount::{mount_target, umount_target};
use crate::name::Name;
use crate::services::{StopMode, start_service, stop_service};
use crate::units::Unit;
use once_cell::sync::Lazy;
use rind_common::error::{report_error, rw_read};
use rind_common::fs_async::{FileWriteMode, queue_file_write};
use std::collections::{HashMap, HashSet};

pub static STORE: Lazy<std::sync::RwLock<Store>> =
  Lazy::new(|| std::sync::RwLock::new(Store::default()));

#[derive(Default)]
pub struct Store {
  pub(crate) units: HashMap<Name, Unit>,
  pub(crate) enabled: HashMap<Name, HashSet<String>>,

  pub(crate) states: HashMap<String, Vec<FlowInstance>>,
}

#[derive(Clone, Copy)]
pub enum PersistMode {
  Yes,
  No,
}

impl Store {
  fn parse_enabled_line(&mut self, line: &str) {
    if let Some((unit_name, rest)) = line.split_once('@') {
      let mut filter = HashSet::default();
      if let Some(inner) = rest.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        for item in inner.split(',').map(str::trim).filter(|x| !x.is_empty()) {
          filter.insert(item.to_string());
        }
      }
      self.enabled.insert(unit_name.into(), filter);
    } else if !line.is_empty() {
      self.enabled.insert(line.into(), HashSet::default());
    }
  }

  fn load_enabled_fallback(&mut self) {
    #[derive(serde::Deserialize)]
    struct FallbackActiveUnits {
      active_units: Vec<String>,
    }

    let config = rw_read(
      &rind_common::config::CONFIG,
      "config read in load_enabled_fallback",
    );
    let path = std::path::Path::new(config.units.fallback.as_str());
    let Ok(content) = std::fs::read_to_string(&path) else {
      return;
    };
    let Ok(fallback) = toml::from_str::<FallbackActiveUnits>(&content) else {
      report_error(
        "fallback active file parse error",
        format!("invalid TOML in {}", path.display()),
      );
      return;
    };
    for line in fallback.active_units {
      self.parse_enabled_line(line.as_str());
    }

    self.save_enabled();
  }

  pub fn insert_unit(&mut self, name: impl Into<Name>, mut unit: Unit) {
    let name = name.into();
    unit.build_index(&name);
    self.units.insert(name, unit);
  }

  pub fn enable_unit(&mut self, name: impl Into<Name>, persist: PersistMode) {
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

    if matches!(persist, PersistMode::Yes) {
      self.save_enabled();
    }
  }

  pub fn disable_unit(&mut self, name: impl Into<Name>, persist: PersistMode) {
    let name = name.into();
    if let Some(ref mut unit) = self.units.get_mut(&name) {
      if let Some(ref mut services) = unit.service {
        for service in services {
          stop_service(service, StopMode::ForceKill);
        }
      }

      if let Some(ref mounts) = unit.mount {
        for mount in mounts {
          umount_target(mount);
        }
      }
    }

    self.enabled.remove(&name);
    if matches!(persist, PersistMode::Yes) {
      self.save_enabled();
    }
  }

  pub fn enable_component(
    &mut self,
    unit_name: impl Into<Name>,
    component: &str,
    persist: PersistMode,
  ) {
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

    if matches!(persist, PersistMode::Yes) {
      self.save_enabled();
    }
  }

  pub fn disable_component(
    &mut self,
    unit_name: impl Into<Name>,
    component: &str,
    persist: PersistMode,
  ) {
    let unit_name = unit_name.into();
    let filter = self.enabled.entry(unit_name.clone()).or_default();
    // filter.exclude.insert(component.to_string());
    filter.remove(component);

    if let Some(unit) = self.units.get_mut(&unit_name) {
      if let Some(services) = &mut unit.service {
        for svc in services {
          if svc.name == component {
            stop_service(svc, StopMode::ForceKill);
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

    if matches!(persist, PersistMode::Yes) {
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
    self.enabled.clear();
    let lines: Vec<serde_json::Value> = self
      .states
      .get("active")
      .map(|items| {
        items
          .iter()
          .filter_map(|inst| inst.payload.get_json_field("name"))
          .collect()
      })
      .unwrap_or_default();
    for line in lines {
      self.parse_enabled_line(&line.to_string());
    }

    if self.enabled.is_empty() {
      self.load_enabled_fallback();
    }
  }

  pub fn save_enabled(&mut self) {
    let mut lines = HashSet::new();
    for (name, filter) in &self.enabled {
      if filter.is_empty() {
        lines.insert(name.to_string());
      } else {
        let mut parts = vec![];
        for inc in filter {
          parts.push(inc.clone());
        }
        lines.insert(format!("{}@{{{}}}", name.to_string(), parts.join(",")));
      }
    }

    let active = self.states.entry("active".to_string()).or_default();

    let mut active_names: Vec<String> = Vec::new();

    active.retain(|instance| {
      let Some(name) = instance.payload.get_json_field("name") else {
        return false;
      };

      let name = name.to_string();

      let is_active = lines.contains(&name);

      if is_active {
        active_names.push(name)
      }

      is_active
    });

    for line in lines {
      if !active_names.contains(&line) {
        active.push(FlowInstance {
          name: "active".to_string(),
          payload: FlowPayload::Json(format!(r#"{{"name": "{}"}}"#, line).into()),
          r#type: crate::flow::FlowType::State,
        });
      }
    }

    self.save_state();
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
    let config = rw_read(&rind_common::config::CONFIG, "config read in load_state");
    let state_path = std::path::Path::new(config.units.state.as_str());
    if let Ok(content) = std::fs::read(&state_path) {
      if let Ok((states, _)) =
        bincode_next::serde::decode_from_slice(&content, bincode_next::config::standard())
      {
        self.states = states;
      } else {
        report_error("load_state decode error", "state file decode failed");
      }
    }
  }

  pub fn save_state(&self) {
    let config = rw_read(&rind_common::config::CONFIG, "config read in save_state");
    let state_path = std::path::Path::new(config.units.state.as_str());

    if let Ok(serialized) =
      bincode_next::serde::encode_to_vec(&self.states, bincode_next::config::standard())
    {
      // currently ineffective because serialization is a bottleneck
      queue_file_write(state_path, serialized, FileWriteMode::Truncate, Some(0o600));
    } else {
      report_error("save_state encode error", "failed to serialize state");
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::flow::{FlowPayload, FlowType};
  use std::collections::HashSet;
  use std::io::Write;

  #[test]
  fn load_enabled_parses_active_state_items() {
    let mut store = Store::default();
    store.states.insert(
      "active".to_string(),
      vec![
        FlowInstance {
          name: "active".to_string(),
          payload: FlowPayload::String("unit_a".to_string()),
          r#type: FlowType::State,
        },
        FlowInstance {
          name: "active".to_string(),
          payload: FlowPayload::String("unit_b@{svc1,svc2}".to_string()),
          r#type: FlowType::State,
        },
      ],
    );

    store.load_enabled();

    assert!(store.enabled.contains_key(&Name::from("unit_a")));
    let filter = store
      .enabled
      .get(&Name::from("unit_b"))
      .cloned()
      .unwrap_or_default();
    assert!(filter.contains("svc1"));
    assert!(filter.contains("svc2"));
  }

  #[test]
  fn save_enabled_keeps_only_declared_active_lines() {
    let mut store = Store::default();
    let mut filter = HashSet::new();
    filter.insert("svc1".to_string());
    store.enabled.insert(Name::from("unit_x"), filter);
    store.states.insert(
      "active".to_string(),
      vec![
        FlowInstance {
          name: "active".to_string(),
          payload: FlowPayload::String("unit_x@{svc1}".to_string()),
          r#type: FlowType::State,
        },
        FlowInstance {
          name: "active".to_string(),
          payload: FlowPayload::String("unit_other".to_string()),
          r#type: FlowType::State,
        },
      ],
    );

    store.save_enabled();

    if let Some(active) = store.states.get("active") {
      assert_eq!(active.len(), 1);
      assert_eq!(active[0].payload.to_string(), "unit_x@{svc1}".to_string());
    } else {
      panic!("expected active state list");
    }
  }

  #[test]
  fn load_enabled_uses_fallback_file_when_active_state_missing() {
    let mut dir = std::env::temp_dir();
    dir.push(format!("rind-fallback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("failed create dir: {e}"));

    let mut file = std::fs::File::create(dir.join("active-fallback.toml"))
      .unwrap_or_else(|e| panic!("failed create fallback file: {e}"));
    writeln!(
      file,
      "active_units = [\"unit_a\", \"unit_b@{{svc1,svc2}}\"]"
    )
    .unwrap_or_else(|e| panic!("failed write fallback: {e}"));

    {
      let mut conf = rind_common::error::rw_write(
        &rind_common::config::CONFIG,
        "config write in test fallback",
      );
      conf.units.path = rind_common::utils::s(dir.to_string_lossy().as_ref());
    }

    let mut store = Store::default();
    store.load_enabled();

    assert!(store.enabled.contains_key(&Name::from("unit_a")));
    let filter = store
      .enabled
      .get(&Name::from("unit_b"))
      .cloned()
      .unwrap_or_default();
    assert!(filter.contains("svc1"));
    assert!(filter.contains("svc2"));

    let _ = std::fs::remove_dir_all(dir);
  }
}
