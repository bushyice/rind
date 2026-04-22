use rind_core::types::Ustr;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UnitSerialized {
  pub name: Ustr,
  pub services: usize,
  pub active_services: usize,
  pub mounts: usize,
  pub mounted: usize,
}

impl UnitSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap_or_default()
  }

  pub fn from_string(str: String) -> Self {
    serde_json::from_str(&str).unwrap_or(Self {
      name: String::new().into(),
      services: 0,
      active_services: 0,
      mounts: 0,
      mounted: 0,
    })
  }

  pub fn many_from_string(str: String) -> Vec<Self> {
    serde_json::from_str(&str).unwrap_or_default()
  }

  pub fn as_some(self) -> Option<Self> {
    Some(self)
  }
}

pub fn serialize_many<T: Serialize>(items: &Vec<T>) -> String {
  serde_json::to_string(items).unwrap_or_default()
}

#[derive(Serialize, Deserialize)]
pub struct ServiceSerialized {
  pub name: Ustr,
  pub last_state: String,
  pub after: Option<Vec<Ustr>>,
  pub restart: bool,
  pub run: Vec<Ustr>,
  pub pid: Option<u32>,
}

impl ServiceSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap_or_default()
  }
}

#[derive(Serialize, Deserialize)]
pub struct StateSerialized {
  pub name: Ustr,
  pub instances: Vec<serde_json::Value>,
  pub keys: Vec<Ustr>,
}

impl StateSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap_or_default()
  }
}

#[derive(Serialize, Deserialize)]
pub struct MountSerialized {
  pub source: Option<Ustr>,
  pub target: Ustr,
  pub fstype: Option<Ustr>,
  pub mounted: bool,
}

#[derive(Serialize, Deserialize)]
pub struct UnitItemsSerialized {
  pub mounts: Vec<MountSerialized>,
  pub services: Vec<ServiceSerialized>,
}

impl UnitItemsSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap_or_default()
  }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PortStateSerialized {
  pub protocol: String,
  pub local_address: String,
  pub local_port: u16,
  pub state: String,
  pub pid: Option<u32>,
  pub process: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetworkStatusSerialized {
  pub interface: Ustr,
  pub method: Ustr,
  pub address: Option<Ustr>,
  pub gateway: Option<Ustr>,
  pub state: Ustr,
}

impl NetworkStatusSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap_or_default()
  }
}

impl PortStateSerialized {
  pub fn stringify(&self) -> String {
    serde_json::to_string(self).unwrap_or_default()
  }
}

#[cfg(test)]
mod tests {
  use super::{ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many};

  #[test]
  fn unit_serialized_roundtrip() {
    let item = UnitSerialized {
      name: "u".to_string().into(),
      services: 2,
      active_services: 1,
      mounts: 1,
      mounted: 1,
    };
    let encoded = item.stringify();
    let decoded = UnitSerialized::from_string(encoded);
    assert_eq!(decoded.name, "u".to_string().into());
    assert_eq!(decoded.services, 2);
  }

  #[test]
  fn invalid_input_falls_back() {
    let decoded = UnitSerialized::from_string("bad-json".to_string());
    assert_eq!(decoded.name, "".to_string().into());
    assert_eq!(
      UnitSerialized::many_from_string("bad-json".to_string()).len(),
      0
    );
  }

  #[test]
  fn serialize_many_and_nested_types() {
    let services = vec![ServiceSerialized {
      name: "svc".to_string().into(),
      last_state: "Active".to_string(),
      after: Some(vec!["db".to_string().into()]),
      restart: true,
      run: vec!["hello".to_string().into()],
      pid: Some(1),
    }];
    let out = serialize_many(&services);
    assert!(!out.is_empty());

    let unit_items = UnitItemsSerialized {
      mounts: vec![],
      services,
    };
    assert!(!unit_items.stringify().is_empty());
  }
}
