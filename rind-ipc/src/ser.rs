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
    serde_json::to_string(self).unwrap_or_default()
  }

  pub fn from_string(str: String) -> Self {
    serde_json::from_str(&str).unwrap_or(Self {
      name: String::new(),
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
    serde_json::to_string(self).unwrap_or_default()
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
    serde_json::to_string(self).unwrap_or_default()
  }
}

#[cfg(test)]
mod tests {
  use super::{ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many};

  #[test]
  fn unit_serialized_roundtrip() {
    let item = UnitSerialized {
      name: "u".to_string(),
      services: 2,
      active_services: 1,
      mounts: 1,
      mounted: 1,
    };
    let encoded = item.stringify();
    let decoded = UnitSerialized::from_string(encoded);
    assert_eq!(decoded.name, "u".to_string());
    assert_eq!(decoded.services, 2);
  }

  #[test]
  fn invalid_input_falls_back() {
    let decoded = UnitSerialized::from_string("bad-json".to_string());
    assert_eq!(decoded.name, "".to_string());
    assert_eq!(
      UnitSerialized::many_from_string("bad-json".to_string()).len(),
      0
    );
  }

  #[test]
  fn serialize_many_and_nested_types() {
    let services = vec![ServiceSerialized {
      name: "svc".to_string(),
      last_state: "Active".to_string(),
      after: Some(vec!["db".to_string()]),
      restart: true,
      args: vec!["-v".to_string()],
      exec: "/bin/svc".to_string(),
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
