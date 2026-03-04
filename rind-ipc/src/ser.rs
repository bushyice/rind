use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UnitSerialized {
  pub name: String,
  pub services: usize,
  pub active_services: usize,
  pub mounts: usize,
  pub mounted: usize,
}

impl UnitSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap()
  }

  pub fn from_string(str: String) -> Self {
    serde_json::from_str(&str).unwrap()
  }

  pub fn many_from_string(str: String) -> Vec<Self> {
    serde_json::from_str(&str).unwrap()
  }

  pub fn as_some(self) -> Option<Self> {
    Some(self)
  }
}

pub fn serialize_many<T: Serialize>(items: &Vec<T>) -> String {
  serde_json::to_string(items).unwrap()
}

#[derive(Serialize, Deserialize)]
pub struct ServiceSerialized {
  pub name: String,
  pub last_state: String,
  pub after: Option<Vec<String>>,
  pub restart: bool,
  pub args: Vec<String>,
  pub exec: String,
  pub pid: Option<u32>,
}

impl ServiceSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap()
  }
}

#[derive(Serialize, Deserialize)]
pub struct MountSerialized {
  pub source: Option<String>,
  pub target: String,
  pub fstype: Option<String>,
  pub mounted: bool,
}

#[derive(Serialize, Deserialize)]
pub struct UnitItemsSerialized {
  pub mounts: Vec<MountSerialized>,
  pub services: Vec<ServiceSerialized>,
}

impl UnitItemsSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap()
  }
}
