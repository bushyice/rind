use crate::mount::Mount;
use crate::name::Name;
use crate::services::Service;
use crate::sockets::Socket;
use crate::store::STORE;
use std::collections::HashMap;

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Unit {
  pub service: Option<Vec<Service>>,
  pub socket: Option<Vec<Socket>>,
  pub mount: Option<Vec<Mount>>,

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

    if let Some(sockets) = &self.socket {
      for (i, sock) in sockets.iter().enumerate() {
        let key = format!("socket@{}", sock.name);
        self.index.insert(key, i);
      }
    }

    if let Some(mounts) = &self.mount {
      for (i, mnt) in mounts.iter().enumerate() {
        let key = format!("mount@{}", mnt.target);
        self.index.insert(key, i);
      }
    }
  }
}

pub fn load_units_from(path: &str) -> Result<(), anyhow::Error> {
  let mut store = STORE.write().unwrap();

  for entry in
    std::fs::read_dir(path).map_err(|e| anyhow::anyhow!("Failed to read services folder: {e}"))?
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
    } else if name == ".enabled" {
      let content =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read unit: {e}"))?;

      store.parse_enabled(&content);
    }
  }

  Ok(())
}

pub fn load_units() -> Result<(), anyhow::Error> {
  load_units_from(&rind_common::config::CONFIG.read().unwrap().services.path)?;
  Ok(())
}
