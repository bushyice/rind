use rind_core::types::Ustr;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
pub struct UnitSerialized {
  pub name: Ustr,
  pub services: usize,
  pub active_services: usize,
  pub mounts: usize,
  pub mounted: usize,
  pub sockets: usize,
  pub active_sockets: usize,
  pub facets: usize,
  pub active_facets: usize,
  pub impulses: usize,
}

impl UnitSerialized {
  pub fn from_bytes(data: &[u8]) -> Self {
    flexbuffers::from_slice(data).unwrap_or(Self {
      name: String::new().into(),
      ..Default::default()
    })
  }

  pub fn many_from_bytes(data: &[u8]) -> Vec<Self> {
    flexbuffers::from_slice(data).unwrap_or_default()
  }

  pub fn as_some(self) -> Option<Self> {
    Some(self)
  }
}

pub fn serialize_many<T: Serialize>(items: &Vec<T>) -> Vec<u8> {
  flexbuffers::to_vec(items).unwrap_or_default()
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
  pub fn serialize(&self) -> Vec<u8> {
    flexbuffers::to_vec(self).unwrap_or_default()
  }
}

#[derive(Serialize, Deserialize)]
pub struct SocketSerialized {
  pub name: Ustr,
  pub listen: String,
  pub r#type: Ustr,
  pub triggers: usize,
  pub active: bool,
}

#[derive(Serialize, Deserialize)]
pub struct FacetSerialized {
  pub name: Ustr,
  pub instances: Vec<Vec<u8>>,
  pub keys: Vec<Ustr>,
}

#[derive(Serialize, Deserialize)]
pub struct ImpulseSerialized {
  pub name: Ustr,
}

#[derive(Serialize, Deserialize)]
pub struct MountSerialized {
  pub source: Option<Ustr>,
  pub target: Ustr,
  pub fstype: Option<Ustr>,
  pub mounted: bool,
}

#[derive(Serialize, Deserialize, Default)]
pub struct UnitItemsSerialized {
  pub mounts: Vec<MountSerialized>,
  pub services: Vec<ServiceSerialized>,
  pub sockets: Vec<SocketSerialized>,
  pub facets: Vec<FacetSerialized>,
  pub impulses: Vec<ImpulseSerialized>,
}

#[derive(Serialize, Deserialize)]
pub struct PermissionSerialized {
  pub name: Ustr,
  pub id: u16,
  pub group: Option<Ustr>,
}

pub trait SerializeSerialized {
  fn serialize(&self) -> Vec<u8>;
}

impl<T: Serialize> SerializeSerialized for T {
  fn serialize(&self) -> Vec<u8> {
    flexbuffers::to_vec(self).unwrap_or_default()
  }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct IpcListPrinter {
  pub r#type: String, // table/list/string
  pub titles: Vec<String>,
  pub keys: Vec<String>,
  pub colors: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct IpcListComponent {
  pub components: Vec<Vec<u8>>,
  pub printer: Option<IpcListPrinter>,
}

impl IpcListComponent {
  pub fn add(&mut self, item: impl SerializeSerialized) {
    self.components.push(item.serialize());
  }

  pub fn with_printer(mut self, printer: IpcListPrinter) -> Self {
    self.printer = Some(printer);
    self
  }
}

#[derive(Serialize, Deserialize)]
pub struct VariableSerialized {
  pub name: Ustr,
  pub default: String,
  pub value: String,
}

pub fn flexbuf_string<V: AsRef<Vec<u8>>>(vec: V) -> String {
  flexbuffers::Reader::get_root(vec.as_ref().as_slice())
    .unwrap()
    .as_str()
    .to_string()
}

#[cfg(test)]
mod tests {

  use super::{
    SerializeSerialized, ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many,
  };

  #[test]
  fn unit_serialized_roundtrip() {
    let item = UnitSerialized {
      name: "u".to_string().into(),
      services: 2,
      active_services: 1,
      mounts: 1,
      mounted: 1,
      ..Default::default()
    };
    let encoded = item.serialize();
    let decoded = UnitSerialized::from_bytes(&encoded);
    assert_eq!(decoded.name, "u".to_string().into());
    assert_eq!(decoded.services, 2);
  }

  #[test]
  fn invalid_input_falls_back() {
    let decoded = UnitSerialized::from_bytes(b"bad-json");
    assert_eq!(decoded.name, "".to_string().into());
    assert_eq!(UnitSerialized::many_from_bytes(b"bad-json").len(), 0);
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
      ..Default::default()
    };
    assert!(!unit_items.serialize().is_empty());
  }
}
