use crate::mount::Mount;
use crate::names::Name;
use crate::services::{Service, Socket};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};

pub trait UnitComponent: Sized {
  fn find_in_unit<'a>(unit: &'a Unit, name: &str) -> Option<&'a Self>;
}

#[derive(serde::Deserialize)]
pub struct Unit {
  pub service: Option<Vec<Service>>,
  pub socket: Option<Vec<Socket>>,
  pub mount: Option<Vec<Mount>>,
}

impl UnitComponent for Unit {
  // placeholder
  fn find_in_unit<'a>(_unit: &'a Unit, _name: &str) -> Option<&'a Self> {
    None
  }
}

pub static UNITS: Lazy<std::sync::RwLock<Units>> =
  Lazy::new(|| std::sync::RwLock::new(Units::default()));

pub fn load_units_from(path: &str) -> Result<(), anyhow::Error> {
  let mut units = UNITS.write().unwrap();

  for entry in
    std::fs::read_dir(path).map_err(|e| anyhow::anyhow!("Failed to read services folder: {e}"))?
  {
    let entry = entry?;
    let path = entry.path();
    let name = path
      .file_name()
      .ok_or(anyhow::anyhow!("Unit file name could not be retrieved"))?
      .to_string_lossy()
      .to_string();

    if entry.file_type()?.is_file() && path.extension().map_or(false, |x| x == "toml") {
      let content =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read unit: {e}"))?;
      let unit: Unit = toml::from_str(&content)?;

      units.insert_unit(name, unit);
    } else if name == ".enabled" {
      let content =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read unit: {e}"))?;

      units.parse_enabled(&content);
    }
  }

  Ok(())
}

pub fn load_units() -> Result<(), anyhow::Error> {
  load_units_from(&crate::config::CONFIG.lock().unwrap().services.path)?;
  Ok(())
}

#[derive(Default)]
pub struct Units {
  units: HashMap<Name, Unit>,
  enabled: HashSet<Name>,
}

impl Units {
  pub fn insert_unit(&mut self, name: impl Into<Name>, unit: Unit) {
    self.units.insert(name.into(), unit);
  }

  pub fn services(&self) -> impl Iterator<Item = (&Name, &Service)> {
    self.units.iter().flat_map(|(name, unit)| {
      unit
        .service
        .iter()
        .flat_map(move |s| s.iter().map(move |svc| (name, svc)))
    })
  }

  pub fn services_mut(&mut self) -> impl Iterator<Item = (&Name, &mut Service)> {
    self.units.iter_mut().flat_map(|(name, unit)| {
      unit
        .service
        .iter_mut()
        .flat_map(move |services| services.iter_mut().map(move |svc| (name, svc)))
    })
  }

  pub fn enabled(&self) -> impl Iterator<Item = &Unit> {
    self.units.iter().filter_map(move |(name, unit)| {
      if self.enabled.contains(name) {
        Some(unit)
      } else {
        None
      }
    })
  }

  pub fn enabled_mut(&mut self) -> impl Iterator<Item = &mut Unit> {
    self.units.iter_mut().filter_map(|(name, unit)| {
      if self.enabled.contains(name) {
        Some(unit)
      } else {
        None
      }
    })
  }

  pub fn parse_enabled(&mut self, content: &str) {
    self.enabled.extend(
      content
        .lines()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| x.into()),
    );
  }

  pub fn save_enabled(&self) {
    let enabled_path =
      std::path::PathBuf::from(&crate::config::CONFIG.lock().unwrap().services.path)
        .join(".enabled");
    std::fs::write(
      enabled_path,
      self
        .enabled
        .iter()
        .map(|x| x.to_string())
        .collect::<Vec<String>>()
        .join("\n"),
    );
  }

  pub fn unit(&self, name: impl Into<Name>) -> Option<&Unit> {
    self.units.get(&name.into())
  }

  pub fn unit_mut(&mut self, name: impl Into<Name>) -> Option<&mut Unit> {
    self.units.get_mut(&name.into())
  }

  pub fn lookup<T: UnitComponent>(&self, name: &str) -> Option<&T> {
    if let Some((unit_name, thing)) = name.split_once('@') {
      let unit = self.units.get(&unit_name.into())?;
      T::find_in_unit(unit, thing)
    } else {
      self
        .units
        .values()
        .find_map(|unit| T::find_in_unit(unit, name.into()))
    }
  }

  pub fn names(&self) -> Vec<&Name> {
    self.units.keys().collect()
  }
}
