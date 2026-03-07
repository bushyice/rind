use crate::flow::{SignalDefinition, StateDefinition};
use crate::mount::Mount;
use crate::name::Name;
use crate::services::Service;
use crate::store::STORE;
use rind_common::error::{rw_read, rw_write};
use std::collections::HashMap;

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Unit {
  pub service: Option<Vec<Service>>,
  pub mount: Option<Vec<Mount>>,
  pub state: Option<Vec<StateDefinition>>,
  pub signal: Option<Vec<SignalDefinition>>,

  #[serde(skip, default)]
  pub index: HashMap<String, usize>,
}

pub trait UnitComponent {
  type Item: 'static;
  fn iter_field(unit: &Unit) -> Box<dyn Iterator<Item = &Self::Item> + '_>;
  fn iter_field_mut<'a>(unit: &'a mut Unit) -> Box<dyn Iterator<Item = &'a mut Self::Item> + 'a>;
  fn item_name(item: &Self::Item) -> &str;
}

impl Unit {
  pub fn build_index(&mut self, name: &Name) {
    self.index.clear();

    if let Some(services) = &mut self.service {
      for (i, svc) in services.iter_mut().enumerate() {
        svc.unit = name.clone();
        let key = format!("service@{}", svc.name);
        self.index.insert(key, i);
      }
    }

    if let Some(mounts) = &self.mount {
      for (i, mnt) in mounts.iter().enumerate() {
        let key = format!("mount@{}", mnt.target);
        self.index.insert(key, i);
      }
    }

    if let Some(states) = &self.state {
      for (i, state) in states.iter().enumerate() {
        let key = format!("state@{}", state.name);
        self.index.insert(key, i);
      }
    }

    if let Some(signals) = &self.signal {
      for (i, sig) in signals.iter().enumerate() {
        let key = format!("signal@{}", sig.name);
        self.index.insert(key, i);
      }
    }
  }
}

pub fn load_units_from(path: &str) -> Result<(), anyhow::Error> {
  let mut store = rw_write(&STORE, "store write in load_units_from");

  for entry in
    std::fs::read_dir(path).map_err(|e| anyhow::anyhow!("Failed to read units folder: {e}"))?
  {
    let entry = entry?;
    let path = entry.path();
    let name = path
      .file_prefix()
      .ok_or(anyhow::anyhow!("Unit file name could not be retrieved"))?
      .to_string_lossy()
      .to_string();

    if entry.file_type()?.is_file() && path.extension().map_or(false, |x| x == "toml") {
      let content =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read unit: {e}"))?;
      let unit: Unit = toml::from_str(&content)?;

      store.insert_unit(name, unit);
    }
  }

  Ok(())
}

pub fn load_units() -> Result<(), anyhow::Error> {
  let config = rw_read(&rind_common::config::CONFIG, "config read in load_units");
  load_units_from(&config.units.path)?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use super::Unit;
  use crate::flow::{FlowDefinitionBase, SignalDefinition, StateDefinition};
  use crate::mount::Mount;
  use crate::name::Name;
  use crate::services::{RestartPolicy, Service, ServiceState};
  use nix::mount::MsFlags;

  #[test]
  fn build_index_registers_all_components() {
    let mut unit = Unit {
      service: Some(vec![Service {
        id: Default::default(),
        unit: Name::from(""),
        name: "svc".to_string(),
        exec: "/bin/true".to_string(),
        args: vec![],
        branching: None,
        after: None,
        start_on: None,
        stop_on: None,
        on_start: None,
        on_stop: None,
        transport: None,
        restart: RestartPolicy::Bool(false),
        child: None,
        state: ServiceState::Inactive,
        retry_count: 0,
        manually_stopped: false,
        stop_time: None,
        branch_instances: HashMap::new(),
      }]),
      mount: Some(vec![Mount {
        source: None,
        target: "/tmp/mnt".to_string(),
        fstype: None,
        flags: MsFlags::empty(),
        data: None,
        create: None,
        after: None,
      }]),
      state: Some(vec![StateDefinition(FlowDefinitionBase {
        name: "st".to_string(),
        ..Default::default()
      })]),
      signal: Some(vec![SignalDefinition(FlowDefinitionBase {
        name: "sig".to_string(),
        ..Default::default()
      })]),
      index: Default::default(),
    };

    unit.build_index(&Name::from("unit"));
    assert!(unit.index.contains_key("service@svc"));
    assert!(unit.index.contains_key("mount@/tmp/mnt"));
    assert!(unit.index.contains_key("state@st"));
    assert!(unit.index.contains_key("signal@sig"));
  }
}
